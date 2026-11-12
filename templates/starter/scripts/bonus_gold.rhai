// A round type written as a script. The file name is the round kind, used
// from rounds.ron as  Custom(kind: "bonus_gold", params: {"gold": 3}).
//
// `begin` runs when the round starts and `finish` when it ends. Run
// `autoengine api` for everything `game` can do.

fn begin(game) {
    game.log("A quiet round. Everyone gets paid when it ends.");
}

fn finish(game) {
    for p in game.players() {
        game.give_gold(p, game.param("gold"));
    }
}
