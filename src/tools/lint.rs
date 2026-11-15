//! The content checker behind `autoengine check`.

use std::collections::BTreeSet;
use std::path::Path;

use crate::content::{ContentSet, RoundKindDef};
use crate::effect::{Condition, Effect, StatusKind, Trigger};
use crate::name::Name;

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
        // An ability the unit can never pay for is dead weight.
        let cost = unit.ability.as_ref().and_then(|a| Some((a, content.resource(a.cost.resource()?)?)));
        if let Some((ability, resource)) = cost
            && let Ok(template) = content.base_template(&unit.id, 1)
        {
            let capacity = template.stats.get(resource.max);
            let short = match ability.cost {
                crate::effect::Cost::Amount { amount, .. } => capacity < amount,
                _ => capacity <= 0,
            };
            if short {
                warn(
                    subject.clone(),
                    format!(
                        "can never cast {}: it has {capacity} {} (the {} stat) at one star",
                        ability.name, resource.id, resource.max
                    ),
                );
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

    // A signal only does something when one thing sends it and another
    // waits for it, and the two are easy to spell differently.
    let passives = content.passives();
    let effects = content.effects();
    let sent: BTreeSet<Name> = effects
        .iter()
        .filter_map(|(_, spec)| match &spec.effect {
            Effect::Signal { name } => Some(*name),
            _ => None,
        })
        .collect();
    let heard: BTreeSet<Name> = passives
        .iter()
        .filter_map(|(_, p)| match p.trigger {
            Trigger::Signal(name) => Some(name),
            _ => None,
        })
        .collect();
    for (owner, passive) in &passives {
        if let Trigger::Signal(name) = passive.trigger
            && !sent.contains(&name)
        {
            warn(owner.clone(), format!("waits for the signal \"{name}\", which nothing in the content sends (a Rust hook still can)"));
        }
    }
    for (owner, spec) in &effects {
        if let Effect::Signal { name } = &spec.effect
            && !heard.contains(name)
        {
            warn(owner.clone(), format!("sends the signal \"{name}\", which nothing waits for"));
        }
    }
    // A condition that asks for a status nothing can apply is never true.
    const BUILT_IN: [&str; 10] = [
        "stunned", "stat-modified", "afflicted", "silenced", "disarmed", "rooted", "taunted", "wounded", "invulnerable", "unstoppable",
    ];
    let marks: BTreeSet<&str> = effects
        .iter()
        .filter_map(|(_, spec)| match &spec.effect {
            Effect::ApplyStatus { status: StatusKind::Marker { name, .. }, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut asked: Vec<(&String, &Condition)> = Vec::new();
    asked.extend(passives.iter().filter_map(|(owner, p)| Some((owner, p.condition.as_ref()?))));
    asked.extend(effects.iter().filter_map(|(owner, spec)| Some((owner, spec.condition.as_ref()?))));
    for (owner, condition) in asked {
        condition.walk(&mut |c| {
            if let Condition::HasStatus { status, .. } = c
                && !BUILT_IN.contains(&status.as_str())
                && !marks.contains(status.as_str())
            {
                warn(owner.clone(), format!("asks for the status \"{status}\", which nothing in the content applies"));
            }
        });
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
    // Every language should name everything. Descriptions that are empty
    // in the content itself need no translation.
    for (language, table) in content.translations() {
        let missing: Vec<String> = content
            .text_keys()
            .into_iter()
            .filter(|key| !table.contains_key(key))
            .filter(|key| {
                let mut parts = key.splitn(3, '.');
                let (kind, id, field) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
                content.text(None, kind, id, field).is_some_and(|own| !own.is_empty())
            })
            .collect();
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(5).map(String::as_str).collect();
            let more = if missing.len() > 5 { format!(" and {} more", missing.len() - 5) } else { String::new() };
            warn(
                format!("language {language}"),
                format!("has no text for {}{more}; the original text shows instead", shown.join(", ")),
            );
        }
    }
    warnings
}
