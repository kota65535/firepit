use axoupdater::AxoUpdater;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Checking for updates...");
    match AxoUpdater::new_for("firepit").load_receipt()?.run_sync()? {
        Some(update) => {
            eprintln!("New release {} installed!", &update.new_version);
        }
        None => {
            eprintln!("Already up to date; not upgrading");
        }
    }
    Ok(())
}
