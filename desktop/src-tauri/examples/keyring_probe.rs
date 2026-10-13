//! Manual keychain check with a dummy credential. Never use a real API key.
//!
//!   cargo run --example keyring_probe -- save
//!   cargo run --example keyring_probe -- load    # new process = app restart
//!   cargo run --example keyring_probe -- delete
//!
//! `load` prints PERSISTED only when the exact dummy value comes back from
//! the operating-system credential store. Run `delete` when finished.

use keyring::Entry;

const SERVICE: &str = "krutrim-mulyankan";
const ACCOUNT: &str = "dummy-probe";
const DUMMY: &str = "km-dummy-credential-for-tests-only";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let entry = Entry::new(SERVICE, ACCOUNT)?;
    match mode.as_str() {
        "save" => {
            entry.set_password(DUMMY)?;
            println!("SAVED dummy credential under {SERVICE}/{ACCOUNT}");
        }
        "load" => match entry.get_password() {
            Ok(secret) if secret == DUMMY => println!("PERSISTED dummy credential matches"),
            Ok(_) => println!("MISMATCH stored value differs"),
            Err(err) => println!("NOT FOUND ({err})"),
        },
        "delete" => {
            entry.delete_credential()?;
            println!("DELETED dummy credential");
        }
        _ => {
            eprintln!("usage: keyring_probe <save|load|delete>");
            std::process::exit(2);
        }
    }
    Ok(())
}
