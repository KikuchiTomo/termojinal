//! Utility to sign jterm commands.
//!
//! Usage:
//!   jterm-sign <command.toml> <signing-key-hex>
//!   jterm-sign --generate-key
//!
//! The signing key is the 64-byte hex-encoded Ed25519 secret key.
//! Use `--generate-key` to create a new keypair.

use ed25519_dalek::SigningKey;
use jterm_ipc::command_signer;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() == 2 && args[1] == "--generate-key" {
        generate_key();
        return;
    }

    if args.len() != 3 {
        eprintln!("Usage: jterm-sign <command.toml> <signing-key-hex>");
        eprintln!("       jterm-sign --generate-key");
        std::process::exit(1);
    }

    let toml_path = &args[1];
    let key_hex = &args[2];

    // Read the TOML file
    let toml_content = match std::fs::read_to_string(toml_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {}", toml_path, e);
            std::process::exit(1);
        }
    };

    // Parse the signing key from hex
    let key_bytes = match hex_decode(key_hex) {
        Some(b) if b.len() == 32 => b,
        _ => {
            eprintln!("Error: signing key must be 64 hex characters (32 bytes)");
            std::process::exit(1);
        }
    };

    let key_array: [u8; 32] = key_bytes.try_into().unwrap();
    let signing_key = SigningKey::from_bytes(&key_array);

    // Sign the content
    let signature = command_signer::sign_command(&toml_content, &signing_key);

    // Check if the TOML already has a signature field and update/add it
    let new_content = if toml_content.contains("\nsignature") || toml_content.starts_with("signature") {
        // Replace existing signature line
        toml_content
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("signature") && trimmed.contains('=') {
                    format!("signature = \"{}\"", signature)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // Append signature field before the end
        let trimmed = toml_content.trim_end();
        format!("{}\nsignature = \"{}\"\n", trimmed, signature)
    };

    // Write back
    if let Err(e) = std::fs::write(toml_path, &new_content) {
        eprintln!("Error writing {}: {}", toml_path, e);
        std::process::exit(1);
    }

    println!("Signed {} successfully.", toml_path);
    println!("Signature: {}", signature);
}

fn generate_key() {
    let (signing_key, pub_key_bytes) = command_signer::generate_keypair();

    let secret_hex: String = signing_key
        .to_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    let public_hex: String = pub_key_bytes.iter().map(|b| format!("{:02x}", b)).collect();

    println!("Generated new Ed25519 keypair:");
    println!();
    println!("Secret key (keep this safe!): {}", secret_hex);
    println!("Public key: {}", public_hex);
    println!();
    println!("To embed the public key in the binary, update JTERM_PUBLIC_KEY in command_signer.rs:");
    print!("const JTERM_PUBLIC_KEY: [u8; 32] = [");
    for (i, b) in pub_key_bytes.iter().enumerate() {
        if i % 8 == 0 {
            print!("\n    ");
        }
        print!("0x{:02x}, ", b);
    }
    println!("\n];");
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}
