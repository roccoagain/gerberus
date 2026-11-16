use std::collections::BTreeMap;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args[0] == "--stats" {
        let mut kinds: BTreeMap<i64, usize> = BTreeMap::new();
        for a in &args[1..] {
            match gerberus::altium::records::read_stream(std::path::Path::new(a), "FileHeader")
                .and_then(|d| gerberus::altium::records::parse_stream(&d.unwrap()))
            {
                Ok(rs) => {
                    for r in rs {
                        *kinds.entry(r.kind()).or_default() += 1
                    }
                }
                Err(e) => println!("ERR {a}: {e}"),
            }
        }
        println!("{kinds:?}");
        return;
    }
    let data = gerberus::altium::records::read_stream(
        std::path::Path::new(&args[0]),
        args.get(1).map(|s| s.as_str()).unwrap_or("FileHeader"),
    )
    .unwrap()
    .unwrap();
    for (i, r) in gerberus::altium::records::parse_stream(&data)
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut kv: Vec<_> = r.fields.iter().collect();
        kv.sort();
        println!(
            "{} {}",
            i as i64 - 1,
            kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("|")
        );
    }
}
