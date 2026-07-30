//! Atılabilir migration uygulayıcı — bu repoda migration'lar ELLE uygulanır
//! (sqlx migrate kullanılmıyor) ve geliştirme makinesinde psql istemcisi yok.
//!
//! Kullanım:
//!   DATABASE_URL=... cargo run -p wf-wfd --example apply_sql -- migrations/wf/X.sql
//!
//! Tüm dosya TEK transaction'da koşar: yarım uygulanmış migration bırakmaz.
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("kullanım: apply_sql <dosya.sql>")?;
    let url = env::var("DATABASE_URL")?;
    let sql = std::fs::read_to_string(&path)?;

    let pool = sqlx::PgPool::connect(&url).await?;
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(&sql).execute(&mut *tx).await?;
    tx.commit().await?;
    println!("uygulandı: {path}");
    Ok(())
}
