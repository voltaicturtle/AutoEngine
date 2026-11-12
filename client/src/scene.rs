//! The 3D view of the board: the camera, picking, and the ground geometry.
//!
//! World space: the board lies on the plane y = 0 with its centre at the
//! origin. A hex has radius one. Rows grow towards +Z, which is the side
//! the camera looks from by default, so the viewer's half is the near one.

use macroquad::models::{Mesh, Vertex, draw_mesh};
use macroquad::prelude::*;

pub const SQRT3: f32 = 1.732_050_8;

/// A camera looking at the board through a rectangle of the window.
pub struct View {
    pub camera: Camera3D,
    /// Where the 3D picture goes, in window coordinates.
    pub area: Rect,
    matrix: Mat4,
    inverse: Mat4,
}

impl View {
    /// A camera orbiting `target`, pulled back just far enough for every
    /// point in `fit` to be on screen. `yaw` turns it around the board and
    /// `zoom` scales the distance.
    pub fn fitted(area: Rect, target: Vec3, fit: &[Vec3], yaw: f32, zoom: f32) -> View {
        let pitch = 46.0_f32.to_radians();
        let direction = vec3(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos());
        let make = |distance: f32| {
            let camera = Camera3D {
                position: target + direction * distance,
                target,
                up: Vec3::Y,
                fovy: 38.0_f32.to_radians(),
                aspect: Some(area.w / area.h.max(1.0)),
                projection: Projection::Perspective,
                render_target: None,
                viewport: Some((
                    area.x as i32,
                    (screen_height() - area.y - area.h) as i32,
                    area.w as i32,
                    area.h as i32,
                )),
                z_near: 0.1,
                z_far: 400.0,
            };
            let matrix = camera.matrix();
            View { camera, area, matrix, inverse: matrix.inverse() }
        };
        let mut distance = 6.0;
        let mut view = make(distance);
        while distance < 120.0 && !fit.iter().all(|p| view.ndc(*p).is_some_and(|n| n.x.abs() < 0.97 && n.y.abs() < 0.96)) {
            distance += 0.5;
            view = make(distance);
        }
        if (zoom - 1.0).abs() > 1e-3 {
            view = make(distance * zoom);
        }
        view
    }

    fn ndc(&self, p: Vec3) -> Option<Vec3> {
        let clip = self.matrix * p.extend(1.0);
        (clip.w > 1e-4).then(|| clip.truncate() / clip.w)
    }

    /// Window position of a world point. Points behind the camera land far
    /// off screen.
    pub fn project(&self, p: Vec3) -> Vec2 {
        match self.ndc(p) {
            Some(n) => vec2(
                self.area.x + (n.x + 1.0) / 2.0 * self.area.w,
                self.area.y + (1.0 - n.y) / 2.0 * self.area.h,
            ),
            None => vec2(-10_000.0, -10_000.0),
        }
    }

    /// How many pixels one world unit covers around `p`.
    pub fn pixels_per_unit(&self, p: Vec3) -> f32 {
        let right = self.camera.up.cross(self.camera.position - self.camera.target).normalize_or_zero();
        self.project(p).distance(self.project(p + right))
    }

    /// The point of the ground plane under a window position.
    pub fn ground_at(&self, screen: Vec2) -> Option<Vec3> {
        if !self.area.contains(screen) {
            return None;
        }
        let x = (screen.x - self.area.x) / self.area.w * 2.0 - 1.0;
        let y = 1.0 - (screen.y - self.area.y) / self.area.h * 2.0;
        let near = self.inverse.project_point3(vec3(x, y, -1.0));
        let far = self.inverse.project_point3(vec3(x, y, 1.0));
        let ray = far - near;
        if ray.y.abs() < 1e-6 {
            return None;
        }
        let t = -near.y / ray.y;
        (t > 0.0).then(|| near + ray * t)
    }

    /// Switches drawing to the 3D camera, with depth testing.
    pub fn begin(&self) {
        set_camera(&self.camera);
    }

    /// Back to flat window coordinates for the interface.
    pub fn end(&self) {
        set_default_camera();
    }
}

/// Collects ground geometry so it goes to the GPU in a few big batches.
#[derive(Default)]
pub struct Ground {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
}

fn shaded(color: Color, shade: f32) -> Color {
    Color::new(color.r * shade, color.g * shade, color.b * shade, color.a)
}

impl Ground {
    fn flush_if_full(&mut self, incoming: usize) {
        if self.vertices.len() + incoming > 8_000 {
            self.draw();
        }
    }

    /// A hexagonal slab with its top at `center.y` and pointy ends along Z.
    pub fn hex_tile(&mut self, center: Vec3, radius: f32, thickness: f32, color: Color) {
        self.flush_if_full(40);
        let corner = |i: usize| {
            let a = (30.0 + 60.0 * i as f32).to_radians();
            center + vec3(a.cos() * radius, 0.0, a.sin() * radius)
        };
        let base = self.vertices.len() as u16;
        self.vertices.push(Vertex::new2(center, Vec2::ZERO, color));
        for i in 0..6 {
            self.vertices.push(Vertex::new2(corner(i), Vec2::ZERO, color));
        }
        for i in 0..6u16 {
            self.indices.extend([base, base + 1 + i, base + 1 + (i + 1) % 6]);
        }
        let down = vec3(0.0, thickness, 0.0);
        for i in 0..6 {
            // Sides get darker the further they face away from the light.
            let a = (60.0 + 60.0 * i as f32).to_radians();
            let side = shaded(color, 0.45 + 0.20 * (a.sin() * 0.5 + 0.5));
            let (p, q) = (corner(i), corner((i + 1) % 6));
            let b = self.vertices.len() as u16;
            for v in [p, q, q - down, p - down] {
                self.vertices.push(Vertex::new2(v, Vec2::ZERO, side));
            }
            self.indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }

    /// A flat ring lying on the ground.
    pub fn ring(&mut self, center: Vec3, inner: f32, outer: f32, color: Color) {
        const STEPS: u16 = 20;
        self.flush_if_full(2 * STEPS as usize);
        let base = self.vertices.len() as u16;
        for i in 0..STEPS {
            let a = std::f32::consts::TAU * i as f32 / STEPS as f32;
            let dir = vec3(a.cos(), 0.0, a.sin());
            self.vertices.push(Vertex::new2(center + dir * inner, Vec2::ZERO, color));
            self.vertices.push(Vertex::new2(center + dir * outer, Vec2::ZERO, color));
        }
        for i in 0..STEPS {
            let (a, b) = (base + i * 2, base + ((i + 1) % STEPS) * 2);
            self.indices.extend([a, a + 1, b + 1, a, b + 1, b]);
        }
    }

    /// A filled circle lying on the ground.
    pub fn disc(&mut self, center: Vec3, radius: f32, color: Color) {
        self.ring(center, 0.0, radius, color);
    }

    pub fn draw(&mut self) {
        if !self.indices.is_empty() {
            draw_mesh(&Mesh {
                vertices: std::mem::take(&mut self.vertices),
                indices: std::mem::take(&mut self.indices),
                texture: None,
            });
        }
    }
}
