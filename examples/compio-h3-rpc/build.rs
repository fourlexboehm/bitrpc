use std::{env, fs, path::PathBuf};

use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=certs/server.crt.pem");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let certs_dir = manifest_dir.join("certs");

    if certs_dir.join("server.crt.pem").exists() {
        // Keep existing certificate to avoid having to re-trust every build.
        return;
    }

    fs::create_dir_all(&certs_dir).expect("create certs dir");

    let mut params = CertificateParams::new(vec!["localhost".into()]);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::KeyCertSign,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let cert = rcgen::Certificate::from_params(params).expect("generate cert");

    fs::write(
        certs_dir.join("server.crt.der"),
        cert.serialize_der().expect("serialize cert"),
    )
    .expect("write cert der");
    fs::write(
        certs_dir.join("server.crt.pem"),
        cert.serialize_pem().expect("serialize pem"),
    )
    .expect("write cert pem");
    fs::write(
        certs_dir.join("server.key.der"),
        cert.serialize_private_key_der(),
    )
    .expect("write key der");
    fs::write(
        certs_dir.join("server.key.pem"),
        cert.serialize_private_key_pem(),
    )
    .expect("write key pem");
}
