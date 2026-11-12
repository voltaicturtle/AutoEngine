//! Particles: small camera-facing squares for sparks, bolts and puffs.
//!
//! Purely decorative and local to the client. The simulation never sees
//! them, so they use their own little random generator.

use macroquad::models::{Mesh, Vertex, draw_mesh};
use macroquad::prelude::*;

/// More than this and the oldest are dropped.
const MAX_PARTICLES: usize = 3_000;

struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: f32,
    color: Color,
    /// Downward acceleration; negative makes the particle float up.
    gravity: f32,
}

/// How a burst of particles looks.
#[derive(Clone, Copy)]
pub struct Burst {
    pub count: usize,
    pub color: Color,
    /// Top speed, in hex radii per second.
    pub speed: f32,
    /// Longest life in seconds.
    pub life: f32,
    pub size: f32,
    pub gravity: f32,
}

pub struct Particles {
    list: Vec<Particle>,
    seed: u32,
}

impl Default for Particles {
    fn default() -> Self {
        Particles { list: Vec::new(), seed: 0x9E37_79B9 }
    }
}

impl Particles {
    /// A number from 0 to 1.
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// Particles flying out from a point in all directions.
    pub fn burst(&mut self, at: Vec3, burst: Burst) {
        for _ in 0..burst.count {
            let dir = vec3(self.random() - 0.5, self.random() - 0.2, self.random() - 0.5).normalize_or_zero();
            let speed = burst.speed * (0.35 + 0.65 * self.random());
            let life = burst.life * (0.5 + 0.5 * self.random());
            let size = burst.size * (0.6 + 0.4 * self.random());
            self.list.push(Particle {
                pos: at,
                vel: dir * speed,
                age: 0.0,
                life,
                size,
                color: burst.color,
                gravity: burst.gravity,
            });
        }
    }

    /// A bolt flying from one point to another in `seconds`, with a short
    /// trail strung out behind it.
    pub fn bolt(&mut self, from: Vec3, to: Vec3, seconds: f32, color: Color) {
        for i in 0..6 {
            let lag = i as f32 * 0.07;
            self.list.push(Particle {
                pos: from,
                vel: (to - from) / seconds * (1.0 - lag),
                age: 0.0,
                life: seconds,
                size: 0.20 - i as f32 * 0.025,
                color: if i == 0 { WHITE } else { color },
                gravity: 0.0,
            });
        }
    }

    pub fn update(&mut self, dt: f32) {
        for p in &mut self.list {
            p.age += dt;
            p.vel.y -= p.gravity * dt;
            p.pos += p.vel * dt;
        }
        self.list.retain(|p| p.age < p.life);
        if self.list.len() > MAX_PARTICLES {
            let extra = self.list.len() - MAX_PARTICLES;
            self.list.drain(..extra);
        }
    }

    /// Draws every particle facing the camera. Call inside the 3D pass.
    pub fn draw(&self, camera: &Camera3D) {
        let forward = (camera.target - camera.position).normalize_or_zero();
        let right = forward.cross(camera.up).normalize_or_zero();
        let up = right.cross(forward);
        for chunk in self.list.chunks(2_000) {
            let mut vertices = Vec::with_capacity(chunk.len() * 4);
            let mut indices = Vec::with_capacity(chunk.len() * 6);
            for p in chunk {
                let fade = 1.0 - p.age / p.life;
                let color = Color::new(p.color.r, p.color.g, p.color.b, p.color.a * fade);
                let (r, u) = (right * p.size * 0.5, up * p.size * 0.5);
                let base = vertices.len() as u16;
                for corner in [-r - u, r - u, r + u, -r + u] {
                    vertices.push(Vertex::new2(p.pos + corner, Vec2::ZERO, color));
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            draw_mesh(&Mesh { vertices, indices, texture: None });
        }
    }
}
