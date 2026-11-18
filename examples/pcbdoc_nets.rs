fn main() {
    let path = std::env::args().nth(1).expect("usage: pcbdoc_nets <board.PcbDoc>");
    let mut lines: Vec<String> = gerberus::altium::pcb::load_pad_nets(std::path::Path::new(&path))
        .unwrap()
        .into_iter()
        .map(|p| format!("{}.{}\t{}", p.reference, p.pad, p.net))
        .collect();
    lines.sort();
    lines.dedup();
    for l in lines {
        println!("{l}");
    }
}
