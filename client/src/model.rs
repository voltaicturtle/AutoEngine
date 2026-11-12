//! 3D models: loading through Assimp, skeletal animation and drawing.
//!
//! Loading is split in two. [`load_geometry`] reads a file into plain data
//! and needs no window, so it can be tested anywhere. [`Model::upload`]
//! then creates the textures, which needs the OpenGL context.
//!
//! A file with animations keeps its node tree, bones and clips (a [`Rig`]);
//! its vertices are skinned on the CPU each time it is drawn. A file
//! without has its node transforms baked in on import. Either way the model
//! is normalised to stand on the ground, centred, one unit tall, facing +Z.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use macroquad::models::{Mesh, Vertex, draw_mesh};
use macroquad::prelude::*;
use russimp::material::{DataContent, Material, PropertyTypeInfo, TextureType};
use russimp::node::Node;
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
    /// Up to four bones (indices into [`Rig::bones`]) and how much each
    /// moves this vertex. All weights zero means the vertex is not skinned.
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl ModelVertex {
    pub fn new(pos: Vec3, normal: Vec3, uv: Vec2, color: [f32; 3]) -> Self {
        ModelVertex { pos, normal, uv, color, joints: [0; 4], weights: [0.0; 4] }
    }
}

/// One mesh with one material.
#[derive(Clone, Debug, Default)]
pub struct RawPart {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u16>,
    /// An encoded image (PNG and the like) for the diffuse texture.
    pub texture: Option<Vec<u8>>,
    /// In a rigged model, the node this mesh hangs from. Vertices without
    /// bone weights follow it.
    pub node: Option<usize>,
}

/// A node of the model's transform tree. Parents come before children.
#[derive(Clone, Debug)]
pub struct RigNode {
    pub name: String,
    pub parent: Option<usize>,
    pub local: Mat4,
}

/// The keys animating one node, with times in seconds.
#[derive(Clone, Debug, Default)]
pub struct Channel {
    pub node: usize,
    pub positions: Vec<(f32, Vec3)>,
    pub rotations: Vec<(f32, Quat)>,
    pub scales: Vec<(f32, Vec3)>,
}

/// A named animation.
#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    /// Length in seconds.
    pub duration: f32,
    pub channels: Vec<Channel>,
}

/// A moment of a clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub clip: usize,
    pub time: f32,
    /// Wrap around at the end, or hold the last frame.
    pub looped: bool,
}

/// Skeleton and animations of a model.
#[derive(Clone, Debug)]
pub struct Rig {
    pub nodes: Vec<RigNode>,
    /// Each bone: the node that drives it, and the matrix taking a vertex
    /// from mesh space into that node's space.
    pub bones: Vec<(usize, Mat4)>,
    pub clips: Vec<Clip>,
    /// Applied after skinning: the normalising move and scale.
    fit: Mat4,
}

fn sample<T: Copy>(keys: &[(f32, T)], time: f32, mix: impl Fn(T, T, f32) -> T) -> Option<T> {
    let first = keys.first()?;
    if time <= first.0 {
        return Some(first.1);
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if time < b.0 {
            let span = (b.0 - a.0).max(1e-6);
            return Some(mix(a.1, b.1, (time - a.0) / span));
        }
    }
    keys.last().map(|k| k.1)
}

impl Rig {
    /// Finds a clip by name, ignoring case; a clip whose name merely
    /// contains `name` ("Armature|Walk") also counts.
    pub fn clip(&self, name: &str) -> Option<usize> {
        let wanted = name.to_lowercase();
        self.clips
            .iter()
            .position(|c| c.name.to_lowercase() == wanted)
            .or_else(|| self.clips.iter().position(|c| c.name.to_lowercase().contains(&wanted)))
    }

    /// World matrix of every node, in the bind pose or at a moment of a clip.
    pub fn globals(&self, pose: Option<Pose>) -> Vec<Mat4> {
        let mut locals: Vec<Mat4> = self.nodes.iter().map(|n| n.local).collect();
        if let Some(pose) = pose
            && let Some(clip) = self.clips.get(pose.clip)
        {
            let time = if pose.looped && clip.duration > 0.0 {
                pose.time.rem_euclid(clip.duration)
            } else {
                pose.time.clamp(0.0, clip.duration)
            };
            for channel in &clip.channels {
                let (scale, rotation, position) = locals[channel.node].to_scale_rotation_translation();
                let position = sample(&channel.positions, time, |a, b, t| a.lerp(b, t)).unwrap_or(position);
                let rotation = sample(&channel.rotations, time, |a, b, t| a.slerp(b, t)).unwrap_or(rotation);
                let scale = sample(&channel.scales, time, |a, b, t| a.lerp(b, t)).unwrap_or(scale);
                locals[channel.node] = Mat4::from_scale_rotation_translation(scale, rotation, position);
            }
        }
        let mut globals: Vec<Mat4> = Vec::with_capacity(locals.len());
        for (i, node) in self.nodes.iter().enumerate() {
            let global = match node.parent {
                Some(parent) => globals[parent] * locals[i],
                None => locals[i],
            };
            globals.push(global);
        }
        globals
    }
}

#[derive(Clone, Debug, Default)]
pub struct RawModel {
    pub parts: Vec<RawPart>,
    pub rig: Option<Rig>,
}

impl RawModel {
    pub fn vertex_count(&self) -> usize {
        self.parts.iter().map(|p| p.vertices.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.indices.len() / 3).sum()
    }

    /// Positions and normals of every part, posed. Without a rig this is
    /// just the stored vertices.
    pub fn posed(&self, pose: Option<Pose>) -> Vec<Vec<(Vec3, Vec3)>> {
        let Some(rig) = &self.rig else {
            return self.parts.iter().map(|p| p.vertices.iter().map(|v| (v.pos, v.normal)).collect()).collect();
        };
        let globals = rig.globals(pose);
        let skin: Vec<Mat4> = rig.bones.iter().map(|(node, offset)| rig.fit * globals[*node] * *offset).collect();
        self.parts
            .iter()
            .map(|part| {
                let rigid = rig.fit * part.node.map_or(Mat4::IDENTITY, |n| globals[n]);
                part.vertices
                    .iter()
                    .map(|v| {
                        let total: f32 = v.weights.iter().sum();
                        let matrix = if total > 0.0 {
                            let mut m = Mat4::ZERO;
                            for (joint, weight) in v.joints.iter().zip(v.weights) {
                                if weight > 0.0 {
                                    m += skin[*joint as usize] * (weight / total);
                                }
                            }
                            m
                        } else {
                            rigid
                        };
                        (matrix.transform_point3(v.pos), matrix.transform_vector3(v.normal).normalize_or_zero())
                    })
                    .collect()
            })
            .collect()
    }

    /// Lowest and highest corner of the model, in its bind pose.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut low = Vec3::splat(f32::MAX);
        let mut high = Vec3::splat(f32::MIN);
        for (pos, _) in self.posed(None).iter().flatten() {
            low = low.min(*pos);
            high = high.max(*pos);
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
        if let Some(rig) = &mut self.rig {
            rig.fit = Mat4::IDENTITY;
        }
        let (low, high) = self.bounds();
        let size = high - low;
        let widest = size.x.max(size.z);
        let scale = 1.0 / size.y.max(widest / 1.4).max(1e-6);
        let origin = vec3((low.x + high.x) / 2.0, low.y, (low.z + high.z) / 2.0);
        match &mut self.rig {
            // A rigged model is posed first and fitted after.
            Some(rig) => rig.fit = Mat4::from_scale(Vec3::splat(scale)) * Mat4::from_translation(-origin),
            None => {
                for v in self.parts.iter_mut().flat_map(|p| &mut p.vertices) {
                    v.pos = (v.pos - origin) * scale;
                }
            }
        }
    }

    /// Adds triangles, splitting them over several parts when they would
    /// not fit one draw call.
    fn push_triangles(&mut self, vertices: &[ModelVertex], triangles: &[[u32; 3]], texture: &Option<Vec<u8>>) {
        self.push_mesh(vertices, triangles, texture, None);
    }

    fn push_mesh(&mut self, vertices: &[ModelVertex], triangles: &[[u32; 3]], texture: &Option<Vec<u8>>, node: Option<usize>) {
        let fresh = || RawPart { texture: texture.clone(), node, ..Default::default() };
        let mut part = fresh();
        let mut remap: HashMap<u32, u16> = HashMap::new();
        for tri in triangles {
            if part.vertices.len() + 3 > MAX_PART_VERTICES || part.indices.len() + 3 > MAX_PART_INDICES {
                self.parts.push(std::mem::replace(&mut part, fresh()));
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
                vertices.push(ModelVertex::new(
                    vec3(r * cos, y, r * sin),
                    vec3(dy * cos, -dr, dy * sin).normalize_or_zero(),
                    Vec2::ZERO,
                    color,
                ));
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

fn import(path: &Path, flatten: bool) -> Result<Scene, String> {
    let file = path.to_str().ok_or_else(|| format!("{}: path is not valid UTF-8", path.display()))?;
    let mut steps = vec![
        PostProcess::Triangulate,
        PostProcess::GenerateSmoothNormals,
        PostProcess::JoinIdenticalVertices,
        PostProcess::SortByPrimitiveType,
        PostProcess::FlipUVs,
        PostProcess::LimitBoneWeights,
    ];
    if flatten {
        steps.push(PostProcess::PreTransformVertices);
    }
    Scene::from_file(file, steps).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a model file with Assimp. Any format Assimp imports will do.
pub fn load_geometry(path: &Path) -> Result<RawModel, String> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()));
    }
    // With animations the node tree has to survive the import; without,
    // let Assimp bake it into the vertices.
    let mut scene = import(path, false)?;
    let animated = !scene.animations.is_empty() && scene.root.is_some();
    if !animated {
        scene = import(path, true)?;
    }
    let folder = path.parent().unwrap_or(Path::new("."));
    let mut model = RawModel::default();
    if animated {
        build_rigged(&scene, folder, &mut model);
    } else {
        for index in 0..scene.meshes.len() {
            let (vertices, triangles, texture) = read_mesh(&scene, index, folder);
            model.push_triangles(&vertices, &triangles, &texture);
        }
    }
    if model.triangle_count() == 0 {
        return Err(format!("{}: the file has no triangles", path.display()));
    }
    model.normalise();
    Ok(model)
}

type MeshData = (Vec<ModelVertex>, Vec<[u32; 3]>, Option<Vec<u8>>);

fn read_mesh(scene: &Scene, index: usize, folder: &Path) -> MeshData {
    let mesh = &scene.meshes[index];
    let material = scene.materials.get(mesh.material_index as usize);
    let base = material.and_then(diffuse_color).unwrap_or([0.8, 0.8, 0.8]);
    let texture = material.and_then(|m| diffuse_texture(m, folder));
    let uvs = mesh.texture_coords.first().and_then(|c| c.as_ref());
    let colors = mesh.colors.first().and_then(|c| c.as_ref());
    let vertices = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let normal = mesh.normals.get(i).map_or(Vec3::Y, |n| vec3(n.x, n.y, n.z));
            let tint = colors.and_then(|c| c.get(i)).map_or([1.0; 3], |c| [c.r, c.g, c.b]);
            ModelVertex::new(
                vec3(p.x, p.y, p.z),
                normal.normalize_or_zero(),
                uvs.and_then(|t| t.get(i)).map_or(Vec2::ZERO, |t| vec2(t.x, t.y)),
                [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2]],
            )
        })
        .collect();
    // Points and lines are left out; only triangles are drawn.
    let triangles = mesh.faces.iter().filter(|f| f.0.len() == 3).map(|f| [f.0[0], f.0[1], f.0[2]]).collect();
    (vertices, triangles, texture)
}

fn matrix(m: &russimp::Matrix4x4) -> Mat4 {
    // Assimp stores rows; glam wants columns.
    Mat4::from_cols_array(&[
        m.a1, m.b1, m.c1, m.d1, m.a2, m.b2, m.c2, m.d2, m.a3, m.b3, m.c3, m.d3, m.a4, m.b4, m.c4, m.d4,
    ])
}

fn build_rigged(scene: &Scene, folder: &Path, model: &mut RawModel) {
    // Flatten the node tree, parents first, noting which nodes carry meshes.
    let mut nodes: Vec<RigNode> = Vec::new();
    let mut placed: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(Rc<Node>, Option<usize>)> = scene.root.iter().map(|r| (r.clone(), None)).collect();
    while let Some((node, parent)) = stack.pop() {
        let index = nodes.len();
        nodes.push(RigNode { name: node.name.clone(), parent, local: matrix(&node.transformation) });
        placed.extend(node.meshes.iter().map(|m| (*m as usize, index)));
        for child in node.children.borrow().iter() {
            stack.push((child.clone(), Some(index)));
        }
    }
    let by_name = |name: &str| nodes.iter().position(|n| n.name == name);

    let mut bones: Vec<(usize, Mat4)> = Vec::new();
    for (mesh_index, node) in placed {
        if mesh_index >= scene.meshes.len() {
            continue;
        }
        let (mut vertices, triangles, texture) = read_mesh(scene, mesh_index, folder);
        let mut influences: Vec<Vec<(u16, f32)>> = vec![Vec::new(); vertices.len()];
        for bone in &scene.meshes[mesh_index].bones {
            let Some(bone_node) = by_name(&bone.name) else { continue };
            let Ok(id) = u16::try_from(bones.len()) else { break };
            bones.push((bone_node, matrix(&bone.offset_matrix)));
            for w in &bone.weights {
                if let Some(list) = influences.get_mut(w.vertex_id as usize)
                    && w.weight > 0.0
                {
                    list.push((id, w.weight));
                }
            }
        }
        for (vertex, mut list) in vertices.iter_mut().zip(influences) {
            list.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (slot, (joint, weight)) in list.into_iter().take(4).enumerate() {
                vertex.joints[slot] = joint;
                vertex.weights[slot] = weight;
            }
        }
        model.push_mesh(&vertices, &triangles, &texture, Some(node));
    }

    let clips = scene
        .animations
        .iter()
        .enumerate()
        .map(|(i, animation)| {
            let per_second = if animation.ticks_per_second > 0.0 { animation.ticks_per_second } else { 25.0 };
            let seconds = |ticks: f64| (ticks / per_second) as f32;
            let channels = animation
                .channels
                .iter()
                .filter_map(|c| {
                    Some(Channel {
                        node: by_name(&c.name)?,
                        positions: c.position_keys.iter().map(|k| (seconds(k.time), vec3(k.value.x, k.value.y, k.value.z))).collect(),
                        rotations: c
                            .rotation_keys
                            .iter()
                            .map(|k| (seconds(k.time), Quat::from_xyzw(k.value.x, k.value.y, k.value.z, k.value.w).normalize()))
                            .collect(),
                        scales: c.scaling_keys.iter().map(|k| (seconds(k.time), vec3(k.value.x, k.value.y, k.value.z))).collect(),
                    })
                })
                .collect();
            let name = if animation.name.is_empty() { format!("clip {}", i + 1) } else { animation.name.clone() };
            Clip { name, duration: seconds(animation.duration), channels }
        })
        .collect();
    model.rig = Some(Rig { nodes, bones, clips, fit: Mat4::IDENTITY });
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

/// A model ready to draw.
pub struct Model {
    raw: RawModel,
    textures: Vec<Option<Texture2D>>,
    /// Height after normalising (one, unless the model is very wide).
    pub height: f32,
}

impl Model {
    /// Creates the textures. Needs the window to exist.
    pub fn upload(mut raw: RawModel) -> Model {
        let height = if raw.vertex_count() == 0 { 1.0 } else { raw.bounds().1.y };
        let textures = raw
            .parts
            .iter_mut()
            .map(|p| {
                p.texture.take().map(|bytes| {
                    let texture = Texture2D::from_file_with_format(&bytes, None);
                    texture.set_filter(FilterMode::Linear);
                    texture
                })
            })
            .collect();
        Model { raw, textures, height }
    }

    /// The clip with this name, if the model is animated and has one.
    pub fn clip(&self, name: &str) -> Option<usize> {
        self.raw.rig.as_ref()?.clip(name)
    }

    /// Length of a clip in seconds.
    pub fn clip_duration(&self, clip: usize) -> f32 {
        self.raw.rig.as_ref().and_then(|r| r.clips.get(clip)).map_or(0.0, |c| c.duration)
    }

    pub fn vertex_count(&self) -> usize {
        self.raw.vertex_count()
    }

    /// Draws the model standing at `at`, turned `yaw` radians around the Y
    /// axis and scaled, in the given pose if it is animated. `tint`
    /// multiplies the colours and `flash` blends them towards white.
    /// Skinning and lighting are worked out here, per vertex.
    pub fn draw(&self, at: Vec3, yaw: f32, scale: f32, tint: [f32; 3], flash: f32, pose: Option<Pose>) {
        let (sin, cos) = yaw.sin_cos();
        let light = LIGHT.normalize();
        let turn = |v: Vec3| vec3(v.x * cos + v.z * sin, v.y, -v.x * sin + v.z * cos);
        let posed = self.raw.posed(pose);
        for ((part, placed), texture) in self.raw.parts.iter().zip(&posed).zip(&self.textures) {
            let vertices = part
                .vertices
                .iter()
                .zip(placed)
                .map(|(v, (pos, normal))| {
                    let shade = AMBIENT + DIFFUSE * turn(*normal).dot(light).max(0.0);
                    let channel = |c: f32, t: f32| {
                        let lit = (c * t * shade).min(1.0);
                        ((lit + (1.0 - lit) * flash) * 255.0) as u8
                    };
                    Vertex {
                        position: at + turn(*pos) * scale,
                        uv: v.uv,
                        color: [channel(v.color[0], tint[0]), channel(v.color[1], tint[1]), channel(v.color[2], tint[2]), 255],
                        normal: Vec4::ZERO,
                    }
                })
                .collect();
            draw_mesh(&Mesh { vertices, indices: part.indices.clone(), texture: texture.clone() });
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
        let vertex = ModelVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO, [1.0; 3]);
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

    #[test]
    fn loads_a_skinned_gltf_with_its_clips() {
        let model = load_geometry(&sample("emberlash_duelist.gltf")).unwrap();
        let rig = model.rig.as_ref().expect("the file is animated");
        for name in ["idle", "walk", "attack", "cast"] {
            let clip = rig.clip(name).unwrap_or_else(|| panic!("clip {name}"));
            assert!(rig.clips[clip].duration > 0.1);
        }
        assert!(rig.clip("WALK").is_some() && rig.clip("dance").is_none());
        assert!(!rig.bones.is_empty());
        assert!(model.parts.iter().flat_map(|p| &p.vertices).all(|v| v.weights.iter().sum::<f32>() > 0.99));
        // The bind pose is normalised like any other model.
        let (low, high) = model.bounds();
        assert!(low.y.abs() < 1e-3 && (high.y - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_clip_moves_the_vertices_its_bones_own() {
        let model = load_geometry(&sample("emberlash_duelist.gltf")).unwrap();
        let rig = model.rig.as_ref().unwrap();
        let attack = rig.clip("attack").unwrap();
        let rest: Vec<Vec3> = model.posed(None).into_iter().flatten().map(|v| v.0).collect();
        let at = |time: f32| -> Vec<Vec3> {
            model.posed(Some(Pose { clip: attack, time, looped: false })).into_iter().flatten().map(|v| v.0).collect()
        };
        let mid = at(rig.clips[attack].duration * 0.5);
        let moved = rest.iter().zip(&mid).filter(|(a, b)| a.distance(**b) > 0.05).count();
        assert!(moved > 0, "the swing moves something");
        assert!(moved < rest.len(), "but not the whole body");
        // Past the end a one-shot clip holds its last frame; a looped one wraps.
        let end = at(rig.clips[attack].duration);
        assert!(end.iter().zip(&at(99.0)).all(|(a, b)| a.distance(*b) < 1e-4));
        let looped = |time: f32| -> Vec<Vec3> {
            model.posed(Some(Pose { clip: attack, time, looped: true })).into_iter().flatten().map(|v| v.0).collect()
        };
        let d = rig.clips[attack].duration;
        assert!(looped(d * 0.5).iter().zip(&looped(d * 1.5)).all(|(a, b)| a.distance(*b) < 1e-3));
        // Normals stay unit length through skinning.
        assert!(model.posed(Some(Pose { clip: attack, time: 0.1, looped: false })).iter().flatten().all(|v| (v.1.length() - 1.0).abs() < 1e-3));
    }
}
