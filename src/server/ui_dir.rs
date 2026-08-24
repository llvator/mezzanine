//! Finding the built browser UI (SRV-008).
//!
//! The UI mount used to be `Path::new("ui/dist")`, resolved against the
//! process working directory. That made `nao watch` a command you had to run
//! from one particular directory, and made `cargo install --git …` produce a
//! binary whose watch mode could never serve a UI — the fallback page told
//! the reader to `cd ui && npm run build` in a repo they had never cloned.
//!
//! So the directory becomes something the user can point at, with the old
//! behaviour kept as the last candidate rather than the only one. Both
//! servers resolve and mount through this module, so `watch` and `serve`
//! cannot drift apart.

use std::path::{Path, PathBuf};

use axum::{response::Html, routing::get, Router};

/// Environment variable equivalent of `--ui-dir`, for wrapper scripts and
/// launchers that have nowhere to put a flag.
const UI_DIR_ENV: &str = "NAO_UI_DIR";

/// Locate the built UI. Resolution order, first hit wins:
///
/// 1. `--ui-dir <path>`,
/// 2. `NAO_UI_DIR`,
/// 3. `ui_dir` in the user settings file (CFG-002) — the place to write it
///    down once, rather than keeping an `export` in a shell profile,
/// 4. a `ui/dist` sibling of the running executable — what an installed
///    layout can satisfy without the repo being present,
/// 5. `./ui/dist` — today's behaviour, kept last so running from the repo
///    root keeps working with no flag.
///
/// A candidate counts as a hit only when it contains an `index.html`. An
/// empty or half-built `dist` would otherwise win the race and serve 404s,
/// which reads as "the UI is broken" rather than "the UI isn't built".
///
/// A `--ui-dir` that does not resolve is an error: the user typed it, and
/// silently falling through to `./ui/dist` is the class of surprise this
/// whole ticket removes. `NAO_UI_DIR` and the settings file only warn,
/// because both outlive the session that set them.
///
/// `from_settings` is passed in rather than read here so the settings file is
/// parsed once per run, by the caller that already needs the rest of it — two
/// reads would mean printing every warning in it twice.
pub fn resolve(
    explicit: Option<&Path>,
    from_settings: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    if let Some(dir) = explicit {
        return match usable(dir) {
            Some(found) => Ok(Some(found)),
            None => Err(format!(
                "--ui-dir {}: no index.html there. Point it at a built UI \
                 (`cd ui && npm run build` writes ui/dist).",
                dir.display()
            )),
        };
    }

    if let Some(from_env) = std::env::var_os(UI_DIR_ENV) {
        let dir = PathBuf::from(from_env);
        match usable(&dir) {
            Some(found) => return Ok(Some(found)),
            None => eprintln!(
                "   ⚠ {UI_DIR_ENV}={} has no index.html — ignoring it.",
                dir.display()
            ),
        }
    }

    if let Some(dir) = from_settings {
        match usable(dir) {
            Some(found) => return Ok(Some(found)),
            None => eprintln!(
                "   ⚠ settings: ui_dir {} has no index.html — ignoring it.",
                dir.display()
            ),
        }
    }

    if let Some(beside_exe) = exe_sibling() {
        if let Some(found) = usable(&beside_exe) {
            return Ok(Some(found));
        }
    }

    Ok(usable(Path::new("ui/dist")))
}

/// `<dir of the running binary>/ui/dist`, when the executable path is
/// knowable at all.
fn exe_sibling() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("ui").join("dist"))
}

/// The canonical form of `dir` if it looks like a built UI, else `None`.
/// Canonicalized so the banner prints somewhere the reader can `ls`.
fn usable(dir: &Path) -> Option<PathBuf> {
    if !dir.join("index.html").is_file() {
        return None;
    }
    Some(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()))
}

/// Serve `ui_dir` as the router's fallback, or a page explaining its absence.
///
/// One function, both servers: `watch` and `serve` differ in their API
/// surface, not in how they host the UI.
pub fn mount(app: Router, ui_dir: Option<&Path>, port: u16) -> Router {
    match ui_dir {
        Some(dir) => app.fallback_service(
            tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true),
        ),
        None => {
            let page = missing_ui_page(port);
            app.route("/", get(move || async move { Html(page) }))
        }
    }
}

/// Report where the UI came from, or that there isn't one.
///
/// The resolved path is printed rather than assumed, because with four
/// candidates "the UI loaded" no longer tells you *which* build you are
/// looking at — a stale `ui/dist` beside the installed binary shadowing the
/// one you just rebuilt is otherwise invisible.
pub fn print_banner_line(port: u16, ui: Option<&Path>) {
    match ui {
        Some(dir) => {
            eprintln!("   UI:             http://localhost:{port}/");
            eprintln!("                   from {}", dir.display());
        }
        None => {
            eprintln!(
                "   UI:             none bundled — http://localhost:{port}/ explains the options"
            );
            eprintln!(
                "                   (pass --ui-dir, or open a UI elsewhere and point it here)"
            );
        }
    }
}

/// What to say when there is no UI to serve.
///
/// The reader here is as likely to be someone who ran
/// `cargo install --git …` as a contributor standing in the repo, so the
/// page cannot assume a checkout. It states what *is* running, gives the
/// API base URL, and names both remedies: bring a UI to the engine, or take
/// the engine's address to a UI that is already open.
fn missing_ui_page(port: u16) -> String {
    let base = format!("http://localhost:{port}");
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>nao — engine running, no UI bundled</title>
<style>
  body {{ font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; margin: 0; padding: 3rem 1.5rem;
          background: #16181d; color: #d7dae0; }}
  main {{ max-width: 40rem; margin: 0 auto; }}
  h1 {{ font-size: 1.25rem; font-weight: 600; margin: 0 0 .25rem; }}
  p.lede {{ color: #9aa0aa; margin: 0 0 2rem; }}
  h2 {{ font-size: .8rem; text-transform: uppercase; letter-spacing: .06em;
        color: #9aa0aa; margin: 2rem 0 .5rem; font-weight: 600; }}
  code, pre {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: .875em; }}
  pre {{ background: #1e2128; border: 1px solid #2b2f38; border-radius: 6px;
         padding: .75rem 1rem; overflow-x: auto; }}
  a {{ color: #7aa2f7; }}
</style>
</head>
<body><main>
  <h1>The nao engine is running.</h1>
  <p class="lede">It has no browser UI bundled with it, so there is nothing to show here.
     The API is live at <code>{base}</code> and answering.</p>

  <h2>Point the engine at a UI</h2>
  <p>If you have a built UI on disk (<code>npm run build</code> in the
     <code>ui/</code> directory writes <code>ui/dist</code>), start the engine with it:</p>
  <pre>nao watch . --port {port} --ui-dir /path/to/ui/dist</pre>
  <p>Or set <code>NAO_UI_DIR</code> once instead of passing the flag every time.</p>

  <h2>Point a UI at the engine</h2>
  <p>The UI is an ordinary static site and can be served from anywhere — it does
     not have to come from this server. Open it, tell it this engine's address
     (<code>{base}</code>), and allow its origin here:</p>
  <pre>nao watch . --port {port} --allow-origin http://localhost:4173</pre>
  <p>Without that flag the engine refuses to answer a page it did not serve,
     because loopback is not a boundary against a browser on the same machine.</p>
</main></body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that passes the `index.html` test, under a fresh temp dir.
    fn built_ui(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nao-ui-dir-test-{name}"));
        let dist = dir.join("dist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();
        dist
    }

    #[test]
    fn ui_dir_explicit_flag_wins() {
        let dist = built_ui("explicit");
        let got = resolve(Some(&dist), None).unwrap();
        assert_eq!(got, Some(dist.canonicalize().unwrap()));
    }

    #[test]
    fn ui_dir_explicit_flag_that_is_not_a_build_is_an_error() {
        let empty = std::env::temp_dir().join("nao-ui-dir-test-empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = resolve(Some(&empty), None).unwrap_err();
        assert!(err.contains("--ui-dir"), "{err}");
    }

    /// The settings file loses to the flag, and wins over the two
    /// last-resort candidates (CFG-002).
    #[test]
    fn ui_dir_settings_ranks_below_the_flag_and_above_the_fallbacks() {
        let flagged = built_ui("settings-flag");
        let from_file = built_ui("settings-file");
        assert_eq!(
            resolve(Some(&flagged), Some(&from_file)).unwrap(),
            Some(flagged.canonicalize().unwrap()),
        );
        assert_eq!(
            resolve(None, Some(&from_file)).unwrap(),
            Some(from_file.canonicalize().unwrap()),
        );
    }

    /// Ambient configuration warns and falls through rather than failing —
    /// a settings file outlives the session that wrote it, exactly like
    /// `NAO_UI_DIR`.
    #[test]
    fn ui_dir_settings_that_is_not_a_build_falls_through() {
        let empty = std::env::temp_dir().join("nao-ui-dir-test-settings-empty");
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(resolve(None, Some(&empty)).is_ok());
    }

    #[test]
    fn ui_dir_needs_an_index_html_not_just_a_directory() {
        let dist = built_ui("no-index");
        std::fs::remove_file(dist.join("index.html")).unwrap();
        assert!(usable(&dist).is_none());
    }

    #[test]
    fn ui_dir_missing_page_names_the_api_url_and_no_cd_ui() {
        let page = missing_ui_page(3200);
        assert!(page.contains("http://localhost:3200"));
        assert!(page.contains("--ui-dir"));
        assert!(page.contains("--allow-origin"));
        assert!(
            !page.contains("cd ui"),
            "the fallback page must not assume the reader has the repo"
        );
    }
}
