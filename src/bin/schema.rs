use firepit::config::ProjectConfig;
use std::fs::File;
use std::io::Write;

fn main() -> anyhow::Result<()> {
    let schema = ProjectConfig::schema()?;
    let mut f = File::create("schema.json")?;
    f.write_all(schema.as_bytes())?;
    Ok(())
}
