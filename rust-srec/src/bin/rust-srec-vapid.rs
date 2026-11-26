fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return Ok(());
    }

    let json = args.iter().any(|a| a == "--json");
    let keypair = srec_notify::web_push::generate_vapid_keypair();

    if json {
        println!(
            "{{\"public_key\":\"{}\",\"private_key\":\"{}\"}}",
            keypair.public_key_b64, keypair.private_key_b64
        );
    } else {
        println!("Public Key: {}", keypair.public_key_b64);
        println!("Private Key: {}", keypair.private_key_b64);
    }

    Ok(())
}

fn print_help() {
    println!("rust-srec-vapid - Generate VAPID keys for Web Push");
    println!();
    println!("Usage:");
    println!("  rust-srec-vapid           # prints 'Public Key:' and 'Private Key:' lines");
    println!("  rust-srec-vapid --json    # prints JSON");
    println!();
    println!("Environment variables to set:");
    println!("  WEB_PUSH_VAPID_PUBLIC_KEY=<Public Key>");
    println!("  WEB_PUSH_VAPID_PRIVATE_KEY=<Private Key>");
    println!("  WEB_PUSH_VAPID_SUBJECT=mailto:admin@localhost");
}
