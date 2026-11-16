//! The content checker behind `autoengine check`.

use std::collections::BTreeSet;
use std::path::Path;

use crate::content::{ContentSet, RoundKindDef};
use crate::effect::{Condition, Effect, Selector, StatusKind, Trigger};
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
        for ability in &unit.abilities {
            let Some(resource) = ability.cost.resource().and_then(|id| content.resource(id)) else { continue };
            let Ok(template) = content.base_template(&unit.id, 1) else { continue };
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
        // With several abilities the first affordable one is cast, so an
        // earlier ability that is always affordable first hides a later one.
        let price = |a: &crate::effect::Ability| {
            let resource = content.resource(a.cost.resource()?)?;
            let capacity = content.base_template(&unit.id, 1).ok()?.stats.get(resource.max);
            Some((resource.id.clone(), match a.cost {
                crate::effect::Cost::Amount { amount, .. } => amount,
                _ => capacity,
            }))
        };
        for (later_at, later) in unit.abilities.iter().enumerate() {
            let hidden_by = unit.abilities[..later_at].iter().find(|earlier| {
                earlier.cooldown_ms == 0
                    && matches!((price(earlier), price(later)), (Some((a, cost)), Some((b, at_least))) if a == b && cost <= at_least)
            });
            if let Some(earlier) = hidden_by {
                warn(
                    subject.clone(),
                    format!(
                        "can never cast {}: {} is listed first, costs no more of the same resource and has no cooldown",
                        later.name, earlier.name
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

    // Reviving only does something to the dead, and few selectors reach them.
    for (owner, spec) in &effects {
        if matches!(spec.effect, Effect::Revive { .. })
            && !matches!(spec.target, Selector::Source | Selector::EventOther | Selector::FallenAllies(_) | Selector::Custom(_))
        {
            warn(owner.clone(), "revives units its selector only picks while they are alive; use FallenAllies, or Source or EventOther in a death passive".into());
        }
    }

    for (owner, passive) in &passives {
        let revives_itself = passive.effects.iter().any(|spec| matches!(spec.effect, Effect::Revive { .. }) && spec.target == Selector::Source);
        if passive.trigger == Trigger::OnDeath && revives_itself && passive.max_procs.is_none() && passive.condition.is_none() {
            warn(owner.clone(), "revives itself every time it dies, so it can never be killed; give the passive max_procs".into());
        }
    }

    // A tile has to get onto the board somehow.
    for tile in content.tiles() {
        let by_effect = effects.iter().any(|(_, spec)| matches!(&spec.effect, Effect::PlaceTile { tile: id, .. } if *id == tile.id));
        let by_round = content.schedule().rounds.iter().any(|r| r.tiles.iter().any(|p| p.tile == tile.id));
        if !by_effect && !by_round {
            warn(format!("tile {}", tile.id), "nothing in the content lays it: no effect and no round (Rust code still can)".into());
        }
    }

    // A currency should come from somewhere and go somewhere, and an action
    // should do something. Scripts and Rust hooks can do either, so these
    // only speak up when the content has neither.
    let has_code = !content.scripts().is_empty();
    let grants = |currency: &str| {
        content.actions().iter().flat_map(|a| &a.gives).any(|g| matches!(g, crate::game::Grant::Currency { currency: c, amount } if c == currency && *amount > 0))
    };
    for currency in content.currencies() {
        let subject = format!("currency {}", currency.id);
        let paid = currency.starting > 0
            || currency.per_round > 0
            || content.augments().any(|a| a.per_round.currencies.get(&currency.id).is_some_and(|n| *n > 0))
            || grants(&currency.id);
        let handled = content.actions().iter().any(|a| content.action_handler(&a.id).is_some());
        if !paid && !has_code && !handled {
            warn(subject.clone(), "nothing in the content gives it to players".into());
        }
        let spent = content.actions().iter().flat_map(|a| &a.prices).any(|p| p.currency == currency.id);
        if !spent && !has_code && !handled {
            warn(subject, "nothing in the content lets players spend it".into());
        }
    }
    for action in content.actions() {
        if action.gives.is_empty() && content.action_handler(&action.id).is_none() && !has_code {
            warn(format!("action {}", action.id), "gives nothing and has no handler, so using it does nothing".into());
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
