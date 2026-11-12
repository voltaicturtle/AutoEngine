//! 3D models: loading through Assimp and drawing.
//!
//! Loading is split in two. [`load_geometry`] reads a file into plain data
//! and needs no window, so it can be tested anywhere. [`Model::upload`]
//! then creates the textures, which needs the OpenGL context.
//!
//! Models are static meshes: node transforms are baked in on import, and
//! skeletal animation is not played. Every model is normalised to stand on
//! the ground, centred, one unit tall, facing +Z.

use std::collections::HashMap;
use std::path::Path;

use macroquad::models::{Mesh, Vertex, draw_mesh};
use macroquad::prelude::*;
use russimp::material::{DataContent, Material, PropertyTypeInfo, TextureType};
use russimp::scene::{PostProcess, Scene};

/// Vertices and indices one draw call may carry. Indices are 16 bit, and
/// the batcher's buffers are sized to match in `main`.
pub const MAX_PART_VERTICES: usize = 30_000;
pub const MAX_PART_INDICES: usize = 60_000;

/// Direction the light travels towards the scene from (normalised in use).
const LIGHT: Vec3 = Vec3::new(0.35, 0.85, 0.40);
const AMBIENT: f32 = 0.50;
const DIFFUSE: f32 = 0.62;

#[derive(Clone, Copy, Debug)]
pub struct ModelVertex {
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
    pub color: [f32; 3],
}

/// One mesh with one material.
#[derive(Clone, Debug, Default)]
pub struct RawPart {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u16>,
    /// An encoded image (PNG and the like) for the diffuse texture.
    pub texture: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct RawModel {
    pub parts: Vec<RawPart>,
}

impl RawModel {
    pub fn vertex_count(&self) -> usize {
        self.parts.iter().map(|p| p.vertices.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.indices.len() / 3).sum()
    }

    /// Lowest and highest corner of the model.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut low = Vec3::splat(f32::MAX);
        let mut high = Vec3::splat(f32::MIN);
        for v in self.parts.iter().flat_map(|p| &p.vertices) {
            low = low.min(v.pos);
            high = high.max(v.pos);
        }
        (low, high)
    }

    /// Moves and scales the model so it stands on y = 0, centred on the Y
    /// axis and one unit tall. Wide models are shrunk further so they stay
    /// on their hex.
    pub fn normalise(&mut self) {
        if self.vertex_count() == 0 {
            return;
        }
        let (low, high) = self.bounds();
        let size = high - low;
        let widest = size.x.max(size.z);
        let scale = 1.0 / size.y.max(widest / 1.4).max(1e-6);
        let origin = vec3((low.x + high.x) / 2.0, low.y, (low.z + high.z) / 2.0);
        for v in self.parts.iter_mut().flat_map(|p| &mut p.vertices) {
            v.pos = (v.pos - origin) * scale;
        }
    }

    /// Adds triangles, splitting them over several parts when they would
    /// not fit one draw call.
    fn push_triangles(&mut self, vertices: &[ModelVertex], triangles: &[[u32; 3]], texture: &Option<Vec<u8>>) {
        let mut part = RawPart { texture: texture.clone(), ..Default::default() };
        let mut remap: HashMap<u32, u16> = HashMap::new();
        for tri in triangles {
            if part.vertices.len() + 3 > MAX_PART_VERTICES || part.indices.len() + 3 > MAX_PART_INDICES {
                self.parts.push(std::mem::replace(&mut part, RawPart { texture: texture.clone(), ..Default::default() }));
                remap.clear();
            }
            for &i in tri {
                let Some(vertex) = vertices.get(i as usize) else { continue };
                let index = *remap.entry(i).or_insert_with(|| {
                    part.vertices.push(*vertex);
                    (part.vertices.len() - 1) as u16
                });
                part.indices.push(index);
            }
        }
        if !part.indices.is_empty() {
            self.parts.push(part);
        }
    }

    /// A stand-in figure for units without a model file.
    pub fn figure(color: Color) -> RawModel {
        let base = [color.r, color.g, color.b];
        let light = [(color.r + 1.0) / 2.0, (color.g + 1.0) / 2.0, (color.b + 1.0) / 2.0];
        let mut model = RawModel::default();
        let body = [(0.0, 0.0), (0.30, 0.0), (0.24, 0.30), (0.30, 0.62), (0.16, 0.76), (0.0, 0.76)];
        let head: Vec<(f32, f32)> = (0..=6)
            .map(|i| {
                let a = std::f32::consts::PI * i as f32 / 6.0;
                (0.17 * a.sin(), 0.86 - 0.17 * a.cos())
            })
            .collect();
        model.lathe(&body, base);
        model.lathe(&head, light);
        model.normalise();
        model
    }

    /// Spins `(radius, height)` points around the Y axis.
    fn lathe(&mut self, profile: &[(f32, f32)], color: [f32; 3]) {
        const SEGMENTS: usize = 12;
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for (i, &(r, y)) in profile.iter().enumerate() {
            let a = profile[i.saturating_sub(1)];
            let b = profile[(i + 1).min(profile.len() - 1)];
            let (dr, dy) = (b.0 - a.0, b.1 - a.1);
            for s in 0..SEGMENTS {
                let (sin, cos) = (std::f32::consts::TAU * s as f32 / SEGMENTS as f32).sin_cos();
                vertices.push(ModelVertex {
                    pos: vec3(r * cos, y, r * sin),
                    normal: vec3(dy * cos, -dr, dy * sin).normalize_or_zero(),
                    uv: Vec2::ZERO,
                    color,
                });
            }
        }
        for ring in 0..profile.len().saturating_sub(1) {
            for s in 0..SEGMENTS {
                let s2 = (s + 1) % SEGMENTS;
                let (a, b) = ((ring * SEGMENTS) as u32, ((ring + 1) * SEGMENTS) as u32);
                triangles.push([a + s as u32, b + s as u32, b + s2 as u32]);
                triangles.push([a + s as u32, b + s2 as u32, a + s2 as u32]);
            }
        }
        self.push_triangles(&vertices, &triangles, &None);
    }
}

/// Reads a model file with Assimp. Any format Assimp imports will do.
pub fn load_geometry(path: &Path) -> Result<RawModel, String> {
    let file = path.to_str().ok_or_else(|| format!("{}: path is not valid UTF-8", path.display()))?;
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()));
    }
    let scene = Scene::from_file(
        file,
        vec![
            PostProcess::Triangulate,
            PostProcess::GenerateSmoothNormals,
            PostProcess::JoinIdenticalVertices,
            PostProcess::PreTransformVertices,
            PostProcess::SortByPrimitiveType,
            PostProcess::FlipUVs,
        ],
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;

    let folder = path.parent().unwrap_or(Path::new("."));
    let mut model = RawModel::default();
    for mesh in &scene.meshes {
        let material = scene.materials.get(mesh.material_index as usize);
        let base = material.and_then(diffuse_color).unwrap_or([0.8, 0.8, 0.8]);
        let texture = material.and_then(|m| diffuse_texture(m, folder));
        let uvs = mesh.texture_coords.first().and_then(|c| c.as_ref());
        let colors = mesh.colors.first().and_then(|c| c.as_ref());
        let vertices: Vec<ModelVertex> = mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let normal = mesh.normals.get(i).map_or(Vec3::Y, |n| vec3(n.x, n.y, n.z));
                let tint = colors.and_then(|c| c.get(i)).map_or([1.0; 3], |c| [c.r, c.g, c.b]);
                ModelVertex {
                    pos: vec3(p.x, p.y, p.z),
                    normal: normal.normalize_or_zero(),
                    uv: uvs.and_then(|t| t.get(i)).map_or(Vec2::ZERO, |t| vec2(t.x, t.y)),
                    color: [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2]],
                }
            })
            .collect();
        // Points and lines are left out; only triangles are drawn.
        let triangles: Vec<[u32; 3]> =
            mesh.faces.iter().filter(|f| f.0.len() == 3).map(|f| [f.0[0], f.0[1], f.0[2]]).collect();
        model.push_triangles(&vertices, &triangles, &texture);
    }
    if model.triangle_count() == 0 {
        return Err(format!("{}: the file has no triangles", path.display()));
    }
    model.normalise();
    Ok(model)
}

fn diffuse_color(material: &Material) -> Option<[f32; 3]> {
    // Classic materials use the diffuse colour, PBR ones (glTF) the base colour.
    ["$clr.diffuse", "$clr.base"].iter().find_map(|key| {
        material.properties.iter().find(|p| p.key == *key).and_then(|p| match &p.data {
            PropertyTypeInfo::FloatArray(c) if c.len() >= 3 => Some([c[0], c[1], c[2]]),
            _ => None,
        })
    })
}

/// The diffuse texture as encoded image bytes: embedded in the model file,
/// or a file next to it.
fn diffuse_texture(material: &Material, folder: &Path) -> Option<Vec<u8>> {
    for kind in [TextureType::Diffuse, TextureType::BaseColor] {
        if let Some(texture) = material.textures.get(&kind)
            && let DataContent::Bytes(bytes) = &texture.borrow().data
        {
            return Some(bytes.clone());
        }
        let file = material.properties.iter().find(|p| p.key == "$tex.file" && p.semantic == kind);
        if let Some(PropertyTypeInfo::String(name)) = file.map(|p| &p.data)
            && !name.starts_with('*')
        {
            match std::fs::read(folder.join(name.replace('\\', "/"))) {
                Ok(bytes) => return Some(bytes),
                Err(e) => eprintln!("texture {name}: {e}"),
            }
        }
    }
    None
}

struct Part {
    vertices: Vec<ModelVertex>,
    indices: Vec<u16>,
    texture: Option<Texture2D>,
}

/// A model ready to draw.
pub struct Model {
    parts: Vec<Part>,
    /// Height after normalising (one, unless the model is very wide).
    pub height: f32,
}

impl Model {
    /// Creates the textures. Needs the window to exist.
    pub fn upload(raw: RawModel) -> Model {
        let height = if raw.vertex_count() == 0 { 1.0 } else { raw.bounds().1.y };
        let parts = raw
            .parts
            .into_iter()
            .map(|p| {
                let texture = p.texture.map(|bytes| {
                    let texture = Texture2D::from_file_with_format(&bytes, None);
                    texture.set_filter(FilterMode::Linear);
                    texture
                });
                Part { vertices: p.vertices, indices: p.indices, texture }
            })
            .collect();
        Model { parts, height }
    }

    /// Draws the model standing at `at`, turned `yaw` radians around the Y
    /// axis and scaled. `tint` multiplies the colours and `flash` blends
    /// them towards white. Lighting is worked out here, per vertex.
    pub fn draw(&self, at: Vec3, yaw: f32, scale: f32, tint: [f32; 3], flash: f32) {
        let (sin, cos) = yaw.sin_cos();
        let light = LIGHT.normalize();
        let turn = |v: Vec3| vec3(v.x * cos + v.z * sin, v.y, -v.x * sin + v.z * cos);
        for part in &self.parts {
            let vertices = part
                .vertices
                .iter()
                .map(|v| {
                    let shade = AMBIENT + DIFFUSE * turn(v.normal).dot(light).max(0.0);
                    let channel = |c: f32, t: f32| {
                        let lit = (c * t * shade).min(1.0);
                        ((lit + (1.0 - lit) * flash) * 255.0) as u8
                    };
                    Vertex {
                        position: at + turn(v.pos) * scale,
                        uv: v.uv,
                        color: [channel(v.color[0], tint[0]), channel(v.color[1], tint[1]), channel(v.color[2], tint[2]), 255],
                        normal: Vec4::ZERO,
                    }
                })
                .collect();
            draw_mesh(&Mesh { vertices, indices: part.indices.clone(), texture: part.texture.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../content/models").join(name)
    }

    #[test]
    fn loads_an_obj_with_material_colours() {
        let model = load_geometry(&sample("ironbark_warden.obj")).unwrap();
        assert!(model.triangle_count() > 100);
        let (low, high) = model.bounds();
        assert!(low.y.abs() < 1e-4, "stands on the ground");
        assert!((high.y - 1.0).abs() < 1e-3, "is one unit tall");
        assert!((low.x + high.x).abs() < 1e-3, "is centred");
        // Three materials, each with its own colour.
        let mut colors: Vec<[u8; 3]> =
            model.parts.iter().map(|p| p.vertices[0].color.map(|c| (c * 255.0) as u8)).collect();
        colors.sort();
        colors.dedup();
        assert_eq!(colors.len(), 3);
        assert!(model.parts.iter().all(|p| p.indices.iter().all(|&i| (i as usize) < p.vertices.len())));
    }

    #[test]
    fn loads_a_ply_with_vertex_colours() {
        let model = load_geometry(&sample("grave_wisp.ply")).unwrap();
        let distinct: std::collections::HashSet<[u8; 3]> = model
            .parts
            .iter()
            .flat_map(|p| &p.vertices)
            .map(|v| v.color.map(|c| (c * 255.0) as u8))
            .collect();
        assert!(distinct.len() > 3, "colours vary from vertex to vertex");
    }

    #[test]
    fn finds_the_texture_next_to_the_model() {
        let model = load_geometry(&sample("moss_golem.obj")).unwrap();
        let textured: Vec<&RawPart> = model.parts.iter().filter(|p| p.texture.is_some()).collect();
        assert!(!textured.is_empty());
        assert!(textured[0].texture.as_ref().unwrap().starts_with(b"\x89PNG"));
        assert!(textured[0].vertices.iter().any(|v| v.uv != Vec2::ZERO));
    }

    #[test]
    fn reports_missing_and_broken_files() {
        assert!(load_geometry(&sample("nothing_here.obj")).is_err());
        assert!(load_geometry(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).is_err());
    }

    #[test]
    fn splits_big_meshes_across_draw_calls() {
        let vertex = ModelVertex { pos: Vec3::ZERO, normal: Vec3::Y, uv: Vec2::ZERO, color: [1.0; 3] };
        let vertices = vec![vertex; 90_000];
        let triangles: Vec<[u32; 3]> = (0..30_000).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
        let mut model = RawModel::default();
        model.push_triangles(&vertices, &triangles, &None);
        assert!(model.parts.len() >= 3);
        assert_eq!(model.triangle_count(), 30_000);
        assert!(model.parts.iter().all(|p| p.vertices.len() <= MAX_PART_VERTICES && p.indices.len() <= MAX_PART_INDICES));
    }

    #[test]
    fn the_stand_in_figure_is_normalised() {
        let model = RawModel::figure(RED);
        let (low, high) = model.bounds();
        assert!(low.y.abs() < 1e-4 && (high.y - 1.0).abs() < 1e-3);
    }
}
