use axoupdater::{AxoUpdater, AxoupdateResult};

fn main() -> AxoupdateResult<()> {
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
