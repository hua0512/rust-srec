use base64::Engine as _;
use p256::ecdsa::SigningKey;
use p256::elliptic_curve::rand_core::OsRng;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return Ok(());
    }

    let json = args.iter().any(|a| a == "--json");

    let signing_key = SigningKey::random(&mut OsRng);
    let private_key_raw = signing_key.to_bytes();
    let public_key_raw = signing_key
        .verifying_key()
        .to_encoded_point(false)
        .to_bytes();

    let public_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key_raw);
    let private_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(private_key_raw);

    if json {
        println!(
            "{{\"public_key\":\"{}\",\"private_key\":\"{}\"}}",
            public_b64, private_b64
        );
    } else {
        println!("Public Key: {}", public_b64);
        println!("Private Key: {}", private_b64);
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
