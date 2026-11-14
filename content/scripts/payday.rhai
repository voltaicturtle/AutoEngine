// A round type written as a script: no fighting, everyone gets paid.
//
// Use it from the schedule (see src/sample.rs) with
//     RoundDef::new("Payday", 3, RoundKindDef::custom("payday").param("gold", 3))
//
// `begin` runs when the round starts and `finish` when it ends. The name
// the script is added under is the round kind.

fn begin(game) {
    game.log("Payday! Gold for everyone when the round ends.");
}

fn finish(game) {
    let base = game.param("gold");
    let poorest = -1;
    for p in game.players() {
        // Later stages pay more.
        game.give_gold(p, base + game.stage());
        if poorest < 0 || game.health(p) < game.health(poorest) {
            poorest = p;
        }
    }
    // Whoever is doing worst also finds an item.
    if poorest >= 0 {
        game.give_random_item(poorest);
        game.log(`${game.name(poorest)} finds an item on payday`);
    }
}
