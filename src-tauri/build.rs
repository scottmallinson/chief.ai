/// Set by the release to say this build is one somebody will install, so a
/// binary that cannot sign in fails here rather than shipping.
const REQUIRE_CLIENT_ID: &str = "CHIEF_REQUIRE_CLIENT_ID";

/// The registration a release bakes in. Read by `option_env!` in `github.rs`.
const GITHUB_CLIENT_ID: &str = "CHIEF_GITHUB_CLIENT_ID";

/// The same, for Outlook. No release sets it yet — Chief has no Entra
/// application registered — so it is tracked but never required.
const MICROSOFT_CLIENT_ID: &str = "CHIEF_MICROSOFT_CLIENT_ID";

fn main() {
    // For the reads *this script* makes, below. `option_env!` in the crate
    // itself needs no help — rustc records an `env-dep:` line for it in the
    // dep-info and cargo honours it, which was checked rather than assumed:
    // changing the id recompiles the crate and repeating it does not. A build
    // script's own `std::env::var` is not tracked that way, so without these
    // the assertion would go stale the moment cargo decided it need not rerun.
    for variable in [REQUIRE_CLIENT_ID, GITHUB_CLIENT_ID, MICROSOFT_CLIENT_ID] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    // The release workflow already refuses to start without the repository
    // variable, but that guard checks the *variable* — it cannot see whether
    // the value reached the compiler. This one is inside the compilation, so
    // there is nothing left between it and the binary. A developer build sets
    // nothing and is unaffected: signing in from source is what the run-time
    // environment variable and the Settings field are for.
    if is_set(REQUIRE_CLIENT_ID) && !is_set(GITHUB_CLIENT_ID) {
        panic!(
            "{GITHUB_CLIENT_ID} is empty or unset while {REQUIRE_CLIENT_ID} is on, so this build \
             would ship unable to sign in to GitHub. Set the repository variable, or unset \
             {REQUIRE_CLIENT_ID} if this is not a release."
        );
    }

    tauri_build::build()
}

/// Whether a variable carries anything, treating whitespace as nothing: an
/// unset repository variable arrives as an empty string rather than as absent.
fn is_set(variable: &str) -> bool {
    std::env::var(variable).is_ok_and(|value| !value.trim().is_empty())
}
