// A rule that listens to the game instead of being a round.
//
// `on_event` is called for everything that happens. `event.kind` is the
// event's name and the other fields depend on it (see GameEvent in
// src/game.rs). Here: the first time a player drops below 40 health, they
// get some gold to mount a comeback.

fn on_event(game, event) {
    if event.kind != "PlayerDamaged" || event.health >= 40 || event.health <= 0 {
        return;
    }
    // `get` and `set` keep numbers for the whole game.
    let key = `second_wind_${event.player}`;
    if game.get(key) == 0 {
        game.set(key, 1);
        game.give_gold(event.player, 8);
        game.log(`${game.name(event.player)} catches a second wind (+8 gold)`);
    }
}
