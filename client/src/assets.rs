//! Loaded art: unit models and item icons, cached and reloaded when their files change.

use super::*;

/// Height of a unit at 100% scale, in hex radii.
pub(crate) const UNIT_HEIGHT: f32 = 1.7;

/// How to draw one kind of unit.
#[derive(Clone)]
pub(crate) struct Look {
    pub(crate) model: Rc<Model>,
    pub(crate) scale: f32,
    pub(crate) yaw: f32,
    /// Whether to play animation clips (a setting).
    pub(crate) animate: bool,
}

impl Look {
    pub(crate) fn height(&self) -> f32 {
        self.model.height * self.scale
    }

    /// A moment of the named clip, if the model has it.
    pub(crate) fn pose(&self, clip: &str, time: f32, looped: bool) -> Option<Pose> {
        self.model.clip(clip).filter(|_| self.animate).map(|clip| Pose { clip, time, looped })
    }
}

/// Models loaded so far. Each file is read once, the first time a unit
/// that uses it comes on screen.
pub(crate) struct Models {
    pub(crate) assets: PathBuf,
    /// Loaded files with the time each was last changed on disk.
    pub(crate) files: HashMap<String, (Option<std::time::SystemTime>, Option<Rc<Model>>)>,
    pub(crate) figures: HashMap<String, Rc<Model>>,
}

/// Pictures for units and items: `icons/<id>.png` in the assets folder.
/// Anything without a picture gets a drawn token instead.
pub(crate) struct Icons {
    pub(crate) assets: PathBuf,
    pub(crate) cache: HashMap<String, (Option<std::time::SystemTime>, Option<Texture2D>)>,
}

impl Icons {
    pub(crate) fn path(&self, id: &str) -> PathBuf {
        self.assets.join("icons").join(format!("{id}.png"))
    }

    pub(crate) fn get(&mut self, id: &str) -> Option<Texture2D> {
        if !self.cache.contains_key(id) {
            let path = self.path(id);
            let texture = std::fs::read(&path).ok().map(|bytes| {
                let texture = Texture2D::from_file_with_format(&bytes, None);
                texture.set_filter(FilterMode::Linear);
                texture
            });
            self.cache.insert(id.to_string(), (theme::modified(&path), texture));
        }
        self.cache[id].1.clone()
    }

    /// Forgets pictures whose files changed, appeared or went away.
    pub(crate) fn drop_changed(&mut self) {
        let assets = self.assets.clone();
        self.cache.retain(|id, (seen, _)| theme::modified(&assets.join("icons").join(format!("{id}.png"))) == *seen);
    }
}

impl Models {
    /// Forgets models whose files changed, so they load again when next
    /// drawn. This is what lets an artist re-export and see the result.
    pub(crate) fn drop_changed(&mut self) {
        let assets = self.assets.clone();
        self.files.retain(|file, (seen, _)| theme::modified(&assets.join(file)) == *seen);
    }

    pub(crate) fn look(&mut self, def: Option<&UnitDef>, name: &str) -> Look {
        let wanted = def.and_then(|d| d.model.as_ref());
        let loaded = wanted.and_then(|m| {
            let assets = &self.assets;
            self.files
                .entry(m.file.clone())
                .or_insert_with(|| {
                    let path = assets.join(&m.file);
                    let model = match model::load_geometry(&path) {
                        Ok(raw) => Some(Rc::new(Model::upload(raw))),
                        Err(e) => {
                            eprintln!("model {e}; drawing a stand-in");
                            None
                        }
                    };
                    (theme::modified(&path), model)
                })
                .1
                .clone()
        });
        let model = loaded.unwrap_or_else(|| {
            self.figures
                .entry(name.to_string())
                .or_insert_with(|| Rc::new(Model::upload(RawModel::figure(unit_color(name)))))
                .clone()
        });
        Look {
            model,
            scale: UNIT_HEIGHT * wanted.map_or(100, |m| m.scale_percent) as f32 / 100.0,
            yaw: (wanted.map_or(0, |m| m.yaw_degrees) as f32).to_radians(),
            animate: true,
        }
    }
}
