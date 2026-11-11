use autoengine::hex::{Hex, HexBoard};

fn main() {
    let board = HexBoard::new(7, 8);
    let a = Hex::from_offset(0, 0);
    let b = Hex::from_offset(6, 7);
    println!(
        "AutoEngine groundwork: {}x{} board, distance from corner to corner = {}",
        board.cols(),
        board.rows(),
        a.distance(b)
    );
}
