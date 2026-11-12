//! Watching a fight: playback of a combat record in the 3D scene, for live games and saved replays.

use super::*;

/// A fight replay in progress.
pub(crate) struct Show {
    pub(crate) playback: Playback,
    /// Draw the board rotated half a turn so our units are at the bottom.
    pub(crate) flipped: bool,
    pub(crate) time_ms: f32,
    pub(crate) title: String,
    pub(crate) done_for_ms: f32,
    /// Which way each unit is turned, as a yaw angle.
    pub(crate) facing: HashMap<UnitId, f32>,
    /// Replay time up to which effects and sounds have been set off.
    pub(crate) fx_seen_ms: u32,
    /// Whether the result has been announced with a sound.
    pub(crate) announced: bool,
}

impl Show {
    pub(crate) fn new(playback: Playback, flipped: bool, title: String) -> Self {
        Show { playback, flipped, time_ms: 0.0, title, done_for_ms: 0.0, facing: HashMap::new(), fx_seen_ms: 0, announced: false }
    }
}

/// Moves the fight on screen forward by one frame, and ends it when it has
/// been over for a moment or Space skips it.
pub(crate) fn advance_show(app: &mut App) {
    let dt_ms = get_frame_time() * 1000.0;
    let skip = app.pressed(Action::Skip);
    let mut finished = false;
    if let Some(show) = &mut app.show {
        show.time_ms += dt_ms;
        show.playback.advance_to(show.time_ms as u32);
        show.playback.forget_flashes_older_than(1500);
        if show.playback.is_finished() {
            show.done_for_ms += dt_ms;
        }
        finished = show.done_for_ms > 1500.0 || skip;
    }
    if finished {
        app.show = None;
        app.particles.clear();
    }
}

/// Watching a saved fight: no server, no shop, just the replay on a loop.
pub(crate) fn draw_replay_viewer(app: &mut App, layout: &Layout) {
    let Some(replay) = &app.replay else { return };
    let (title, events) = (replay.title.clone(), replay.record.events.len());
    if app.show.is_none() {
        app.show = Some(Show::new(Playback::new(&replay.record), false, title.clone()));
    }
    let (w, h) = (screen_width(), screen_height());
    panel(0.0, 0.0, w, 44.0, ui().panel);
    label("Replay", 16.0, 29.0, 22.0, ui().text);
    text_centered(&title, w / 2.0, 29.0, 22.0, ui().accent);
    panel(0.0, 44.0, layout.left_w, h - 44.0, ui().panel);
    panel(w - layout.right_w, 44.0, layout.right_w, h - 44.0, ui().panel);
    let x = w - layout.right_w + 14.0;
    label("Saved fight", x, 74.0, 22.0, ui().text);
    let file = app.options.replay.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    draw::wrapped(&file, x, 86.0, layout.right_w - 28.0, 16.0, ui().muted);
    label(&format!("{events} events"), x, 150.0, 16.0, ui().muted);
    for (i, line) in ["Space: restart", "Right-drag: turn the camera", "Scroll: zoom", "Esc: menu", "F3: statistics"].iter().enumerate() {
        label(line, x, 190.0 + i as f32 * 22.0, 16.0, ui().muted);
    }
    advance_show(app);
    draw_replay(app, layout);
}

pub(crate) fn draw_replay(app: &mut App, layout: &Layout) {
    let Some(mut show) = app.show.take() else { return };
    let view = &layout.view;
    let now = show.playback.time_ms();
    let my_team = if show.flipped { 1 } else { 0 };
    let flipped = show.flipped;
    let place = |q: f32, r: f32| {
        let p = layout.axial_world(q, r);
        if flipped { layout.flip(p) } else { p }
    };
    let world_of = |playback: &Playback, id| {
        playback.unit(id).map(|u| {
            let (q, r) = u.position_at(now);
            place(q, r)
        })
    };
    let yaw_towards = |from: Vec3, to: Vec3| {
        let d = to - from;
        (d.x.abs() + d.z.abs() > 1e-3).then(|| d.x.atan2(d.z))
    };

    // Which way everyone faces: where they walk, then whom they hit.
    for u in show.playback.units().filter(|u| u.alive) {
        let ahead = if u.team.0 == my_team { std::f32::consts::PI } else { 0.0 };
        let facing = show.facing.entry(u.id).or_insert(ahead);
        if u.pos != u.prev_pos
            && now.saturating_sub(u.moved_at_ms) < autoengine::playback::MOVE_ANIMATION_MS
            && let Some(yaw) = yaw_towards(place(u.prev_pos.q as f32, u.prev_pos.r as f32), place(u.pos.q as f32, u.pos.r as f32))
        {
            *facing = yaw;
        }
    }
    // Attacks: turn to the target and lunge at it; hits flash white.
    let mut lunge: HashMap<UnitId, Vec3> = HashMap::new();
    let mut hit: HashMap<UnitId, f32> = HashMap::new();
    // Seconds since each unit last attacked.
    let mut attacked: HashMap<UnitId, f32> = HashMap::new();
    for (age, flash) in show.playback.recent_flashes(900) {
        match flash {
            Flash::Attack { source, target } => {
                if let (Some(a), Some(b)) = (world_of(&show.playback, *source), world_of(&show.playback, *target))
                    && let Some(yaw) = yaw_towards(a, b)
                {
                    let since = attacked.entry(*source).or_insert(f32::MAX);
                    *since = since.min(age as f32 / 1000.0);
                    if age < 400 {
                        show.facing.insert(*source, yaw);
                    }
                    if age < 180 {
                        let reach = (std::f32::consts::PI * age as f32 / 180.0).sin() * 0.35;
                        lunge.insert(*source, (b - a).normalize_or_zero() * reach);
                    }
                }
            }
            Flash::Damage { target, amount, .. } if age < 140 && *amount > 0 => {
                let strength = 0.55 * (1.0 - age as f32 / 140.0);
                let entry = hit.entry(*target).or_insert(0.0);
                *entry = entry.max(strength);
            }
            _ => {}
        }
    }

    // Set off particles and sounds for what happened since the last frame.
    let fresh = now.saturating_sub(show.fx_seen_ms).min(300);
    show.fx_seen_ms = now;
    let chest = vec3(0.0, 0.9, 0.0);
    let mut cues: Vec<Sfx> = Vec::new();
    for (age, flash) in show.playback.recent_flashes(fresh) {
        if age >= fresh {
            continue;
        }
        use autoengine::effect::DamageKind;
        let particles = app.settings.particles;
        match flash {
            Flash::Attack { source, target } => {
                if let (Some(a), Some(b)) = (world_of(&show.playback, *source), world_of(&show.playback, *target))
                    && a.distance(b) > 2.2
                    && particles
                {
                    app.particles.bolt(a + chest, b + chest, 0.12, app.theme.effects.bolt);
                }
            }
            Flash::Damage { target, amount, kind, crit, .. } => {
                cues.push(Sfx::Hit);
                if let Some(at) = world_of(&show.playback, *target).filter(|_| particles && *amount > 0) {
                    let effects = app.theme.effects;
                    let mut burst = match kind {
                        DamageKind::Physical => effects.hit_physical,
                        DamageKind::Magic => effects.hit_magic,
                        DamageKind::True => effects.hit_true,
                    };
                    if *crit {
                        burst.count = burst.count * effects.crit_percent as usize / 100;
                    }
                    app.particles.burst(at + chest, burst);
                }
            }
            Flash::Heal { target, .. } => {
                cues.push(Sfx::Heal);
                if let Some(at) = world_of(&show.playback, *target).filter(|_| particles) {
                    app.particles.burst(at + chest * 0.5, app.theme.effects.heal);
                }
            }
            Flash::Cast { unit } => {
                cues.push(Sfx::Cast);
                if let Some(at) = world_of(&show.playback, *unit).filter(|_| particles) {
                    app.particles.burst(at + chest, app.theme.effects.cast);
                }
            }
            Flash::Death { unit } => {
                cues.push(Sfx::Death);
                if let Some(at) = world_of(&show.playback, *unit).filter(|_| particles) {
                    app.particles.burst(at + chest * 0.6, app.theme.effects.death);
                }
            }
        }
    }
    if show.playback.is_finished() && !show.announced {
        show.announced = true;
        if app.state.seat.is_some() {
            cues.push(if show.playback.winner().is_some_and(|t| t.0 == my_team) { Sfx::Victory } else { Sfx::Defeat });
        }
    }
    cues.dedup();
    for cue in cues {
        app.play(cue);
    }

    // The 3D scene.
    view.begin();
    let mut ground = Ground::default();
    board_tiles(&mut ground, layout, false, None);
    for u in show.playback.units().filter(|u| u.alive) {
        let (q, r) = u.position_at(now);
        let at = place(q, r) + RING_LIFT;
        if app.settings.shadows {
            ground.disc(at - RING_LIFT * 0.5, 0.48, ui().shadow);
        }
        ground.ring(at, 0.50, 0.62, if u.team.0 == my_team { ui().friend } else { ui().enemy });
        if u.casting {
            let pulse = 0.72 + 0.06 * (now as f32 / 90.0).sin();
            ground.ring(at, pulse, pulse + 0.10, ui().accent);
        }
    }
    for (age, flash) in show.playback.recent_flashes(450) {
        if let Flash::Cast { unit } = flash
            && let Some(at) = world_of(&show.playback, *unit)
        {
            let t = age as f32 / 450.0;
            let radius = 0.7 + t * 1.3;
            ground.ring(at + RING_LIFT * 2.0, radius, radius + 0.14 * (1.0 - t), Color::new(0.45, 0.72, 1.0, 1.0));
        }
    }
    ground.draw();
    for u in show.playback.units().filter(|u| u.alive) {
        let (q, r) = u.position_at(now);
        let at = place(q, r) + lunge.get(&u.id).copied().unwrap_or(Vec3::ZERO);
        let look = app.look_of_name(&u.name);
        let yaw = show.facing.get(&u.id).copied().unwrap_or(0.0);
        let stunned = u.stunned_until_ms > now;
        let tint = if stunned { [0.62, 0.62, 0.70] } else { [1.0; 3] };
        // Animated models act out what the unit is doing.
        let seconds = now as f32 / 1000.0;
        let walking = u.pos != u.prev_pos && now.saturating_sub(u.moved_at_ms) < autoengine::playback::MOVE_ANIMATION_MS;
        let swing = attacked.get(&u.id).and_then(|age| look.pose("attack", *age, false)).filter(|p| p.time < look.model.clip_duration(p.clip));
        let pose = if stunned {
            look.pose("idle", 0.0, false)
        } else if u.casting {
            look.pose("cast", seconds, true)
        } else if swing.is_some() {
            swing
        } else if walking {
            look.pose("walk", seconds, true)
        } else {
            look.pose("idle", seconds + u.id.0 as f32 * 0.37, true)
        };
        app.draw_model(&look, at, yaw, look.scale, tint, hit.get(&u.id).copied().unwrap_or(0.0), pose);
    }
    app.particles.draw(&view.camera);
    view.end();

    // Flat markers on top: health, numbers, lines.
    text_centered(&show.title, layout.board_mid_x, view.area.y + 24.0, 22.0, ui().text);
    let head_of = |playback: &Playback, id| {
        playback.unit(id).zip(world_of(playback, id)).map(|(u, at)| {
            let height = app.look_of_name(&u.name).height();
            (view.project(at + vec3(0.0, height + 0.25, 0.0)), view.project(at + vec3(0.0, height * 0.55, 0.0)), view.pixels_per_unit(at))
        })
    };
    for u in show.playback.units().filter(|u| u.alive) {
        let Some((head, _, size)) = head_of(&show.playback, u.id) else { continue };
        let full = u.max_health.max(1) as f32;
        draw_health_bar(head, (size * 1.1).clamp(28.0, 70.0), u.health as f32 / full, u.shield as f32 / full, u.team.0 == my_team);
        if u.stunned_until_ms > now {
            text_centered("stun", head.x, head.y - 8.0, 14.0, ui().gold);
        }
    }

    for (age, flash) in show.playback.recent_flashes(900) {
        let age = age as f32;
        match flash {
            Flash::Attack { source, target } if age < 160.0 => {
                if let (Some((_, a, _)), Some((_, b, _))) = (head_of(&show.playback, *source), head_of(&show.playback, *target)) {
                    let alpha = 1.0 - age / 160.0;
                    draw_line(a.x, a.y, b.x, b.y, 2.0, Color::new(1.0, 1.0, 1.0, alpha * 0.6));
                }
            }
            Flash::Damage { target, amount, absorbed, kind, crit } => {
                if let Some((p, _, _)) = head_of(&show.playback, *target) {
                    let alpha = (1.0 - age / 900.0).max(0.0);
                    let mut color = match kind {
                        autoengine::effect::DamageKind::Physical => Color::new(1.0, 0.55, 0.30, alpha),
                        autoengine::effect::DamageKind::Magic => Color::new(0.55, 0.65, 1.0, alpha),
                        autoengine::effect::DamageKind::True => Color::new(1.0, 1.0, 1.0, alpha),
                    };
                    let shown = amount + absorbed;
                    if *amount == 0 {
                        color = Color::new(0.8, 0.8, 0.8, alpha);
                    }
                    // Spread numbers out a little so simultaneous hits stay readable.
                    let jitter = ((shown * 37) % 56) as f32 - 28.0;
                    let size = if *crit { 24.0 } else { 17.0 };
                    text_centered(&shown.to_string(), p.x + jitter, p.y - 12.0 - age * 0.03, size, color);
                }
            }
            Flash::Heal { target, amount } => {
                if let Some((_, p, _)) = head_of(&show.playback, *target) {
                    let alpha = (1.0 - age / 900.0).max(0.0);
                    text_centered(&format!("+{amount}"), p.x, p.y - age * 0.03, 17.0, Color::new(0.4, 1.0, 0.5, alpha));
                }
            }
            Flash::Death { unit } if age < 500.0 => {
                if let Some(at) = world_of(&show.playback, *unit) {
                    let p = view.project(at + vec3(0.0, 0.4, 0.0));
                    let alpha = 1.0 - age / 500.0;
                    let s = view.pixels_per_unit(at) * 0.4;
                    let c = Color::new(0.9, 0.3, 0.3, alpha);
                    draw_line(p.x - s, p.y - s, p.x + s, p.y + s, 4.0, c);
                    draw_line(p.x - s, p.y + s, p.x + s, p.y - s, 4.0, c);
                }
            }
            _ => {}
        }
    }

    let secs = (now as f32 / 1000.0).min(show.playback.duration_ms() as f32 / 1000.0);
    let footer_y = layout.scene_bottom() - 14.0;
    if show.playback.is_finished() {
        let result = match show.playback.winner() {
            Some(team) if app.state.seat.is_none() => format!("Team {} wins", team.0 + 1),
            Some(team) if team.0 == my_team => "Victory".to_string(),
            Some(_) => "Defeat".to_string(),
            None => "Draw".to_string(),
        };
        text_centered(&result, layout.board_mid_x, footer_y, 36.0, ui().gold);
    } else {
        text_centered(&format!("{secs:.1}s   (Space to skip)"), layout.board_mid_x, footer_y, 18.0, ui().muted);
    }
    app.show = Some(show);
}
