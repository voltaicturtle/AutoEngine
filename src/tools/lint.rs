//! The content checker behind `autoengine check`.

use std::path::Path;

use crate::content::{ContentSet, RoundKindDef};

/// Something in the content that loads but is probably a mistake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// What the warning is about, such as `unit ashen_hound`.
    pub subject: String,
    pub message: String,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.subject, self.message)
    }
}

/// Looks for likely mistakes in content that passed validation: things that
/// can never be reached, files that are missing, rounds nothing implements.
/// `assets` is the folder model files are looked up in, if they should be
/// checked.
pub fn lint(content: &ContentSet, assets: Option<&Path>) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let mut warn = |subject: String, message: String| warnings.push(Warning { subject, message });
    let rules = content.rules();

    // A game with no models at all is using stand-ins on purpose.
    let any_models = content.units().any(|u| u.model.is_some());
    for unit in content.units() {
        let subject = format!("unit {}", unit.id);
        if unit.in_shop {
            let cost = unit.cost as usize;
            let offered = cost >= 1 && rules.levels.iter().any(|l| l.shop_odds.get(cost - 1).is_some_and(|w| *w > 0));
            if !offered {
                warn(subject.clone(), format!("costs {} but no player level has shop odds for that cost, so it never appears", unit.cost));
            }
            if unit.traits.is_empty() {
                warn(subject.clone(), "is sold in the shop but has no traits".into());
            }
        }
        match (&unit.model, assets) {
            (Some(model), Some(dir)) if !dir.join(&model.file).is_file() => {
                warn(subject.clone(), format!("model file {} is missing; clients will draw a stand-in", model.file));
            }
            (None, Some(_)) if any_models => warn(subject.clone(), "has no model; clients will draw a stand-in".into()),
            _ => {}
        }
    }

    for def in content.traits() {
        let subject = format!("trait {}", def.id);
        let members = content.units().filter(|u| u.in_shop && u.traits.contains(&def.id)).count() as u32;
        let granted = content.items().filter(|i| i.grants_traits.contains(&def.id)).count() as u32;
        if members == 0 {
            warn(subject.clone(), "no unit in the shop has it".into());
        }
        // Each item can add the trait to one more unit.
        for breakpoint in &def.breakpoints {
            if breakpoint.count > members + granted {
                warn(
                    subject.clone(),
                    format!("the breakpoint at {} cannot be reached: only {members} shop units have the trait", breakpoint.count),
                );
            }
        }
    }

    let scripted: Vec<&String> = content.scripts().keys().collect();
    for round in &content.schedule().rounds {
        if let RoundKindDef::Custom { kind, .. } = &round.kind
            && !scripted.contains(&kind)
        {
            warn(
                format!("round {}", round.name),
                format!("uses the custom kind \"{kind}\", which no script defines; the game's code must register it"),
            );
        }
    }
    warnings
}
