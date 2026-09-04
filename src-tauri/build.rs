fn main() {
    // `option_env!` is read while this crate is compiled, and cargo cannot see
    // that on its own — an environment variable a macro reads is not part of
    // the fingerprint unless the build script says so. Without these two lines
    // a tree compiled once without the variable is reused when it is set, and
    // the release ships a binary whose sign-in button reports that this build
    // has no client id. Which is exactly what REC-60 described.
    println!("cargo:rerun-if-env-changed=CHIEF_GITHUB_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=CHIEF_MICROSOFT_CLIENT_ID");

    tauri_build::build()
}
