//! The asset viewer (`--view FILE`): one model with its statistics, animation clips and the particle effects.

use super::*;

/// The asset viewer: one model file on a turntable.
pub(crate) struct Viewer {
    pub(crate) path: PathBuf,
    pub(crate) model: Option<Rc<Model>>,
    pub(crate) error: Option<String>,
    pub(crate) seen: Option<std::time::SystemTime>,
    pub(crate) reloads: u32,
    /// The clip being played, and when it was started.
    pub(crate) clip: Option<usize>,
    pub(crate) clip_started: f64,
    pub(crate) turning: bool,
    pub(crate) angle: f32,
}

impl Viewer {
    pub(crate) fn new(path: PathBuf) -> Self {
        let mut viewer =
            Viewer { path, model: None, error: None, seen: None, reloads: 0, clip: None, clip_started: 0.0, turning: true, angle: 0.6 };
        viewer.load();
        viewer.reloads = 0;
        viewer
    }

    pub(crate) fn load(&mut self) {
        self.seen = theme::modified(&self.path);
        self.reloads += 1;
        match model::load_geometry(&self.path) {
            Ok(raw) => {
                let model = Model::upload(raw);
                // Keep playing the same clip if the new file still has it.
                self.clip = self.clip.filter(|c| *c < model.info().clips.len());
                self.model = Some(Rc::new(model));
                self.error = None;
            }
            // Keep showing the last good version while the file is broken.
            Err(e) => self.error = Some(e),
        }
    }
}

/// The asset viewer: a model on a turntable with its numbers beside it.
pub(crate) fn draw_asset_viewer(app: &mut App) {
    let (w, h) = (screen_width(), screen_height());
    let side = 330.0;
    let area = Rect::new(0.0, 44.0, w - side, h - 44.0);
    let fit: Vec<Vec3> = [-1.3, 1.3]
        .iter()
        .flat_map(|x| [-1.3, 1.3].iter().flat_map(move |z| [0.0, UNIT_HEIGHT + 0.5].iter().map(move |y| vec3(*x, *y, *z))))
        .collect();
    let view = scene::View::fitted(area, vec3(0.0, UNIT_HEIGHT * 0.5, 0.0), &fit, app.camera_yaw, app.camera_zoom);

    // Keys: digits pick a clip, T stops the turntable, letters fire effects.
    let digits = [KeyCode::Key1, KeyCode::Key2, KeyCode::Key3, KeyCode::Key4, KeyCode::Key5, KeyCode::Key6, KeyCode::Key7, KeyCode::Key8, KeyCode::Key9];
    let effects = app.theme.effects;
    let chest = vec3(0.0, UNIT_HEIGHT * 0.6, 0.0);
    let mut fired: Option<Sfx> = None;
    for (key, burst, sfx) in [
        (KeyCode::H, effects.hit_physical, Sfx::Hit),
        (KeyCode::M, effects.hit_magic, Sfx::Hit),
        (KeyCode::U, effects.hit_true, Sfx::Hit),
        (KeyCode::E, effects.heal, Sfx::Heal),
        (KeyCode::C, effects.cast, Sfx::Cast),
        (KeyCode::D, effects.death, Sfx::Death),
    ] {
        if is_key_pressed(key) {
            app.particles.burst(chest, burst);
            fired = Some(sfx);
        }
    }
    if is_key_pressed(KeyCode::B) {
        app.particles.bolt(vec3(-3.0, chest.y, 0.0), chest, 0.25, effects.bolt);
    }
    if let Some(sfx) = fired {
        app.play(sfx);
    }
    let animate = app.settings.animations;
    let dt = get_frame_time();
    let Some(viewer) = &mut app.viewer else { return };
    if is_key_pressed(KeyCode::T) {
        viewer.turning = !viewer.turning;
    }
    if viewer.turning {
        viewer.angle += dt * 0.7;
    }
    let info = viewer.model.as_ref().map(|m| m.info()).unwrap_or_default();
    if is_key_pressed(KeyCode::Key0) {
        viewer.clip = None;
    }
    for (i, key) in digits.iter().enumerate() {
        if is_key_pressed(*key) && i < info.clips.len() {
            viewer.clip = Some(i);
            viewer.clip_started = get_time();
        }
    }

    view.begin();
    let mut ground = Ground::default();
    ground.hex_tile(Vec3::ZERO, 1.25, 0.22, ui().tile_own);
    if app.settings.shadows {
        ground.disc(RING_LIFT * 0.5, 0.48, ui().shadow);
    }
    ground.ring(RING_LIFT, 0.50, 0.62, ui().friend);
    ground.draw();
    if let Some(model) = &viewer.model {
        let pose = viewer.clip.filter(|_| animate).map(|clip| Pose { clip, time: (get_time() - viewer.clip_started) as f32, looped: true });
        model.draw(Vec3::ZERO, viewer.angle, UNIT_HEIGHT, [1.0; 3], 0.0, pose);
        app.drawn.set((1, info.vertices));
    }
    app.particles.draw(&view.camera);
    view.end();

    panel(0.0, 0.0, w, 44.0, ui().panel);
    label("Asset viewer", 16.0, 29.0, 22.0, ui().text);
    let name = viewer.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    text_centered(&name, (w - side) / 2.0, 29.0, 22.0, ui().accent);
    panel(w - side, 0.0, side, h, ui().panel);
    let x = w - side + 16.0;
    let y = Cell::new(40.0);
    let line = |text: &str, size: f32, color: Color| {
        label(text, x, y.get(), size, color);
        y.set(y.get() + size + 6.0);
    };
    line("Model", 22.0, ui().text);
    line(&format!("{} triangles, {} vertices", info.triangles, info.vertices), 16.0, ui().muted);
    line(&format!("{} draw calls, {} textures", info.parts, info.textures), 16.0, ui().muted);
    line(&format!("{} bones, {} nodes", info.bones, info.nodes), 16.0, ui().muted);
    let status = if viewer.reloads > 0 { format!("reloaded {} times", viewer.reloads) } else { "watching the file for changes".to_string() };
    line(&status, 16.0, ui().muted);
    y.set(y.get() + 10.0);
    line("Animation clips", 22.0, ui().text);
    if info.clips.is_empty() {
        line("none in this file", 16.0, ui().muted);
    }
    line("0  bind pose", 16.0, if viewer.clip.is_none() { ui().gold } else { ui().muted });
    for (i, (clip, seconds)) in info.clips.iter().enumerate().take(9) {
        let color = if viewer.clip == Some(i) { ui().gold } else { ui().muted };
        line(&format!("{}  {clip}  ({seconds:.2}s)", i + 1), 16.0, color);
    }
    y.set(y.get() + 10.0);
    line("Effects", 22.0, ui().text);
    for hint in ["H  physical hit", "M  magic hit", "U  true hit", "E  heal", "C  cast", "D  death", "B  bolt"] {
        line(hint, 16.0, ui().muted);
    }
    y.set(y.get() + 10.0);
    line("View", 22.0, ui().text);
    for hint in ["T  stop or start turning", "Right-drag  turn the camera", "Scroll  zoom"] {
        line(hint, 16.0, ui().muted);
    }
    if let Some(error) = &viewer.error {
        draw_rectangle(10.0, h - 70.0, w - side - 20.0, 60.0, Color::new(0.45, 0.12, 0.12, 0.95));
        draw::wrapped(error, 20.0, h - 66.0, w - side - 40.0, 16.0, ui().text);
    }
}
