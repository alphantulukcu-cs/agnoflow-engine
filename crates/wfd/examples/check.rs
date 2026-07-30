//! Atılabilir WFD doğrulayıcı — dosyaları upload ETMEDEN validator'dan geçirir.
//! WFC cross-WFD kuralları için verilen TÜM dosyalar bir katalog gibi davranır.
//! Kullanım: cargo run -p wf-wfd --example check -- a.json b.json
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{self, WfdProvider};

struct Catalog(Vec<Wfd>);
impl WfdProvider for Catalog {
    fn resolve(&self, wfd_id: &str, version: Option<&str>) -> Option<Wfd> {
        self.0
            .iter()
            .find(|w| w.id == wfd_id && version.map_or(true, |v| w.version == v))
            .cloned()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let mut docs = Vec::new();
    for p in &paths {
        docs.push((p.clone(), Wfd::from_json(&std::fs::read_to_string(p)?)?));
    }
    let catalog = Catalog(docs.iter().map(|(_, w)| w.clone()).collect());

    let mut bad = 0;
    for (path, wfd) in &docs {
        let report = validator::validate_with(wfd, Some(&catalog));
        println!("\n=== {path}  ({}, v{})", wfd.id, wfd.version);
        if report.errors.is_empty() {
            println!("  HATA YOK");
        } else {
            bad += report.errors.len();
            for e in &report.errors {
                println!("  HATA [{}] {}: {}", e.code, e.path, e.message);
            }
        }
        for w in &report.warnings {
            println!("  uyari [{}] {}: {}", w.code, w.path, w.message);
        }
    }
    if bad > 0 {
        std::process::exit(1);
    }
    Ok(())
}
