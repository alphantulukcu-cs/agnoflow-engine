//! Atılabilir sorgu aracı — yerel psql olmadan test DB'ye SELECT atmak için.
//! Tüm kolonları `::text`'e cast ederek sor: `SELECT a::text, b::text FROM ...`
//! Kullanım: DATABASE_URL=... cargo run -p wf-wfd --example q -- "SELECT ..."
use sqlx::Row;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sql = std::env::args().nth(1).ok_or("kullanım: q <sql>")?;
    let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    let rows = sqlx::query(&sql).fetch_all(&pool).await?;
    println!("({} satır)", rows.len());
    for row in rows {
        let n = row.len();
        let cells: Vec<String> = (0..n)
            .map(|i| row.try_get::<Option<String>, _>(i).ok().flatten().unwrap_or_else(|| "-".into()))
            .collect();
        println!("{}", cells.join(" | "));
    }
    Ok(())
}
