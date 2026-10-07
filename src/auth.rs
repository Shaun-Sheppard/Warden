use anyhow::{anyhow, Result};

const SERVICE: &str = "prr-azure-devops";
const ACCOUNT: &str = "pat";

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| anyhow!("Could not open the macOS Keychain: {e}"))
}

pub fn store_pat(pat: &str) -> Result<()> {
    entry()?
        .set_password(pat)
        .map_err(|e| anyhow!("Could not store the PAT in the Keychain: {e}"))
}

pub fn get_pat() -> Result<String> {
    match entry()?.get_password() {
        Ok(pat) => Ok(pat),
        Err(keyring::Error::NoEntry) => Err(anyhow!(
            "No Personal Access Token found. Run `prr auth login` first."
        )),
        Err(e) => Err(anyhow!("Could not read the PAT from the Keychain: {e}")),
    }
}

/// Returns false if there was nothing to remove.
pub fn delete_pat() -> Result<bool> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(anyhow!("Could not remove the PAT from the Keychain: {e}")),
    }
}
