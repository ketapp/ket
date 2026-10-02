//! Launching a command the way the person at the keyboard would launch it.
//!
//! An agent used to be spawned directly: `exec` the binary, hand it the pty,
//! done. That is correct only for commands that are files on `PATH`, and a
//! surprising share of what people actually type is not. `claude-personal` may
//! be a zsh alias; `claude` may be a function, or an `nvm`/`mise`/`asdf` shim
//! that only exists once a shell has sourced its own configuration. Spawning
//! those directly fails with "no such file or directory", and no amount of
//! settings UI makes an alias into an executable.
//!
//! So ket does what a terminal emulator does: it starts the user's login shell
//! and types the command at its first prompt. The shell reads `.zshenv`,
//! `.zprofile`, `.zshrc` and `.zlogin` exactly as it would have, and by the
//! time the line runs, every alias, function and shim the person has is in
//! scope. What they configure in settings is a *command line*, not a filename.
//!
//! # How the line gets typed
//!
//! Not by writing bytes at the pty and hoping. A shell that has not started its
//! line editor yet may still have the tty in canonical mode, and a `.zshrc`
//! that touches `stty` can flush whatever is queued — losing the command with
//! no trace. The command travels in an environment variable instead, and a hook
//! installed inside the shell puts it in the editor buffer once the shell is
//! genuinely ready.
//!
//! For zsh that hook is a `zle-line-init` widget, which is the first point at
//! which the line editor exists. Reaching it needs code that runs *after* the
//! user's own startup files, and zsh offers exactly one way in: `ZDOTDIR`. The
//! wrapper written by [`ensure_wrappers`] is a single `.zshenv` that hands
//! `ZDOTDIR` straight back to the user's own on its first lines — so every
//! later file, `/etc/zshrc` included, is read from where it always was — and
//! arms a `precmd` hook that installs the widget at the first prompt.
//!
//! The handback-first shape is the part worth keeping: a wrapper that stays
//! `ZDOTDIR` for the whole of startup makes `/etc/zshrc` derive `HISTFILE`
//! inside the wrapper directory, and hides later files from any user config
//! that ends in `emulate sh`.
//!
//! Shells other than zsh fall back to `-i -c <line>`, which still sources the
//! user's interactive configuration and still expands aliases. The difference
//! is that the pane closes when the agent exits rather than dropping back to a
//! prompt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::error::{KetError, Result};
use crate::paths;

/// Environment variable carrying the line the shell should run.
///
/// Read and unset by the generated wrapper, so nothing the agent spawns can
/// inherit it and run the line a second time.
pub const STARTUP_COMMAND_ENV: &str = "KET_SHELL_STARTUP_COMMAND";

/// Environment variable carrying the `ZDOTDIR` the wrapper must hand back.
pub const ORIGINAL_ZDOTDIR_ENV: &str = "KET_ORIG_ZDOTDIR";

/// How long a shell may take to answer a probe before it is treated as absent.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Which shell ket is starting, as far as its launch contract is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// zsh, which gets the `ZDOTDIR` wrapper and a real prompt after the agent.
    Zsh,
    /// Everything else, which gets `-i -c <line>`.
    Other,
}

impl ShellKind {
    /// Classifies a shell by the name of its executable.
    pub fn of(shell: &str) -> Self {
        let name = std::path::Path::new(shell)
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match name.as_str() {
            "zsh" => ShellKind::Zsh,
            _ => ShellKind::Other,
        }
    }
}

/// How to start a shell: what to run, with which arguments and extra variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLaunch {
    /// The shell executable.
    pub command: String,
    /// Arguments that make it a login/interactive shell.
    pub args: Vec<String>,
    /// Variables the wrapper needs, merged over the caller's own.
    pub env: BTreeMap<String, String>,
}

/// The user's login shell, or `/bin/sh` when the environment does not say.
///
/// `$SHELL` rather than a hard-coded path, for the same reason a plain terminal
/// pane uses it: a shell that is not the one someone configured has the wrong
/// aliases, the wrong prompt and the wrong history.
pub fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/// Quotes one argument so a shell reads it as a single word.
///
/// Single quotes, because inside them a POSIX shell expands nothing at all. An
/// embedded quote has to leave and re-enter, which is what `'\''` does.
pub fn quote(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/@:=+,".contains(c))
    {
        return argument.to_owned();
    }
    format!("'{}'", argument.replace('\'', "'\\''"))
}

/// Builds the command line for a command and its arguments.
///
/// `command` is shell text and is passed through untouched — that is what lets
/// it be an alias, a function, or a small pipeline. The arguments are separate
/// words the user did not write shell syntax in, so they are quoted.
pub fn line(command: &str, arguments: &[String]) -> String {
    std::iter::once(command.trim().to_owned())
        .chain(arguments.iter().map(|argument| quote(argument)))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// How to start `shell` so that it runs `startup` at its first prompt.
///
/// A `startup` of `None` is a plain interactive shell, which needs no wrapper
/// and gets none: a terminal pane a person opened for themselves should be
/// indistinguishable from the one their terminal emulator would have given
/// them.
pub fn launch(shell: &str, startup: Option<&str>) -> ShellLaunch {
    launch_in(wrapper_root().ok().as_deref(), shell, startup)
}

/// The same, against an explicit wrapper root.
///
/// Kept separate so callers and tests can point the generated tree somewhere
/// throwaway without changing process-global environment — which under a
/// parallel test runner is a race, and in edition 2024 requires `unsafe`, which
/// this workspace forbids. `None` means there is nowhere to write one, and is
/// treated exactly as a failed write.
pub fn launch_in(root: Option<&Path>, shell: &str, startup: Option<&str>) -> ShellLaunch {
    let Some(startup) = startup else {
        return ShellLaunch {
            command: shell.to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
        };
    };

    match ShellKind::of(shell) {
        ShellKind::Zsh => zsh_launch(root, shell, startup),
        // `-i` and not just `-c`: aliases are only expanded by an interactive
        // shell, and the interactive startup files are where a person's `PATH`
        // manipulation lives. The pane exits with the agent here, which is the
        // one thing zsh gets that the fallback does not.
        ShellKind::Other => ShellLaunch {
            command: shell.to_owned(),
            args: vec!["-i".to_owned(), "-c".to_owned(), startup.to_owned()],
            env: BTreeMap::new(),
        },
    }
}

/// The zsh launch, falling back to `-i -c` when the wrapper cannot be written.
///
/// A `ZDOTDIR` pointed at a directory ket failed to populate would make zsh
/// skip the user's configuration entirely — every alias and every `PATH` entry
/// gone. Losing the prompt-after-exit is the smaller loss by a wide margin.
fn zsh_launch(root: Option<&Path>, shell: &str, startup: &str) -> ShellLaunch {
    let fallback = || ShellLaunch {
        command: shell.to_owned(),
        args: vec!["-i".to_owned(), "-c".to_owned(), startup.to_owned()],
        env: BTreeMap::new(),
    };
    let Some(root) = root else {
        return fallback();
    };
    let Ok(root) = ensure_wrappers_in(root) else {
        return fallback();
    };

    let mut env = BTreeMap::new();
    env.insert("ZDOTDIR".to_owned(), root.join("zsh").display().to_string());
    // Only when the parent actually had one. An empty value would be handed
    // back as an empty `ZDOTDIR`, which zsh reads as the current directory.
    if let Some(original) = std::env::var("ZDOTDIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        env.insert(ORIGINAL_ZDOTDIR_ENV.to_owned(), original);
    }
    env.insert(STARTUP_COMMAND_ENV.to_owned(), startup.to_owned());

    ShellLaunch {
        command: shell.to_owned(),
        args: vec!["-l".to_owned()],
        env,
    }
}

/// Where the generated wrapper tree lives.
///
/// Under the cache directory, not the data directory: it is derived from the
/// build and regenerated whenever it differs, so it is not the owner's state
/// and nothing is lost by deleting it. See [`paths::cache_dir`].
fn wrapper_root() -> Result<PathBuf> {
    Ok(paths::cache_dir()?.join("shell"))
}

/// Writes the wrapper tree if it is missing or out of date, returning its root.
///
/// Rewritten whenever the bytes differ rather than on a version counter: the
/// wrapper is generated, so the content *is* the version, and a build that
/// changes it cannot forget to bump anything.
pub fn ensure_wrappers() -> Result<PathBuf> {
    ensure_wrappers_in(&wrapper_root()?)
}

/// The same, under an explicit root.
pub fn ensure_wrappers_in(root: &Path) -> Result<PathBuf> {
    let zshenv = root.join("zsh").join(".zshenv");

    if std::fs::read_to_string(&zshenv).is_ok_and(|existing| existing == ZSH_WRAPPER) {
        return Ok(root.to_path_buf());
    }

    let directory = zshenv.parent().expect("the wrapper path has a parent");
    std::fs::create_dir_all(directory).map_err(|e| KetError::io(directory, e))?;
    std::fs::write(&zshenv, ZSH_WRAPPER).map_err(|e| KetError::io(&zshenv, e))?;
    Ok(root.to_path_buf())
}

/// Whether `name` resolves to something the user's shell could run.
///
/// `PATH` first, because that answers for most commands without starting a
/// process. Only when that fails is the shell asked — an alias or a function
/// exists nowhere on disk, and the shell is the only thing that knows.
///
/// Answers are cached for the life of the process. A probe is a whole shell
/// startup, and this is called on the path that opens a pane.
pub fn probe(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    if let Some(found) = walk_path(name) {
        return Some(found);
    }

    let cache = probe_cache();
    if let Some(cached) = cache.lock().ok()?.get(name) {
        return cached.clone();
    }
    let found = ask_shell(name);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(name.to_owned(), found.clone());
    }
    found
}

/// Forgets every cached shell probe.
///
/// What the Refresh button in settings is for: a person who has just edited
/// their `.zshrc` to add the alias needs the next probe to actually run.
pub fn clear_probe_cache() {
    if let Ok(mut cache) = probe_cache().lock() {
        cache.clear();
    }
    if let Ok(mut cached) = login_path_cache().lock() {
        *cached = None;
    }
    if let Ok(mut cached) = seen_by_cache().lock() {
        cached.clear();
    }
}

/// The `PATH` the user's login shell would run with.
///
/// [`probe`] answers for one name and costs a whole shell startup when `PATH`
/// alone cannot — which, inside `ket.app`, is nearly always. A GUI application
/// inherits launchd's environment, where `PATH` is `/usr/bin:/bin:/usr/sbin:
/// /sbin` and nothing else: Homebrew, mise, nvm and every editor's own "install
/// the shell command" step put their binaries somewhere that list has never
/// heard of. Running from a terminal hides this completely, which is why it is
/// worth stating.
///
/// So: ask once for the whole `PATH` rather than once per name. A caller with a
/// *list* to check — every editor in [`crate::surface::PROFILES`], say — pays a
/// single shell startup and then only `stat`s, instead of one startup per miss.
///
/// Cached for the life of the process, and cleared by [`clear_probe_cache`]
/// alongside the per-name answers it belongs with. Falls back to the process's
/// own `PATH` if the shell cannot be asked, which is the right answer when ket
/// *was* started from a terminal.
pub fn login_path() -> Vec<PathBuf> {
    let cache = login_path_cache();
    if let Ok(cached) = cache.lock()
        && let Some(found) = cached.as_ref()
    {
        return found.clone();
    }
    let found = ask_shell_path().unwrap_or_else(process_path);
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(found.clone());
    }
    found
}

/// Where `binary` lives on the login shell's `PATH`, if anywhere.
///
/// The bulk counterpart to [`probe`]: no shell is started for this particular
/// name, so asking about twenty commands costs what asking about one does. It
/// therefore sees only real files — an alias or a shell function exists nowhere
/// on disk and needs [`probe`].
pub fn find_on_login_path(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let candidate = PathBuf::from(binary);
        return is_executable(&candidate).then_some(candidate);
    }
    login_path()
        .iter()
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

/// The value `binary` would find in `variable` if `line` were typed at the
/// login shell's prompt.
///
/// What [`probe`] cannot answer. A launch line is shell text, and shell text
/// can set variables on its way to the binary: `claude-personal` may be an
/// alias for `CLAUDE_CONFIG_DIR=~/.claude-personal claude`, after which every
/// session that agent writes lands in a directory ket's own environment never
/// names. Parsing the alias would be a guess at shell syntax. Asking the
/// shell is the answer — and the shell inherits ket's environment, so a value
/// exported to ket is reported too, unless the line overrides it.
///
/// `binary` is never run. The shell is handed a function of that name which
/// prints the variable, and the line is typed with that function in scope, so
/// an alias or a wrapper function expands exactly as it would at a real
/// prompt and stops at the function. A line whose first word is a file — a
/// path, or a name `command -v` resolves to one — is not typed at all, since
/// nothing there could be intercepted; the shell's own exported value stands.
/// `env` is set for the probe shell on top of ket's own: a guard the binary
/// honours, so that a wrapper which reaches it anyway makes it exit rather
/// than start.
///
/// `None` is "unset, empty, or the shell could not be asked". The three mean
/// the same to a caller, which is to fall back on the default. Cached per
/// line for the life of the process, and cleared by [`clear_probe_cache`].
pub fn variable_seen_by(
    line: &str,
    binary: &str,
    variable: &str,
    env: &[(&str, &str)],
) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || !is_plain_word(binary) || !is_variable_name(variable) {
        return None;
    }

    let key = format!("{variable} {binary} {line}");
    // Held across the probe on purpose. The second caller — the window's
    // thread, typically, arriving while a warm-up thread is still asking —
    // waits for that answer instead of starting a shell of its own.
    let mut cache = seen_by_cache().lock().ok()?;
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }

    let env = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let found = run_in_shell(seen_by_script(line, binary, variable), env, "variable")
        .as_deref()
        .and_then(seen_by_value);
    cache.insert(key, found.clone());
    found
}

fn login_path_cache() -> &'static Mutex<Option<Vec<PathBuf>>> {
    static CACHE: OnceLock<Mutex<Option<Vec<PathBuf>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// This process's own `PATH`, split into directories.
fn process_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Asks an interactive login shell to print its `PATH`.
fn ask_shell_path() -> Option<Vec<PathBuf>> {
    let text = run_in_shell("printf %s \"$PATH\"".to_owned(), Vec::new(), "PATH")?;

    // A prompt framework that prints on startup would otherwise end up in the
    // list as a directory name. `$PATH` is the last line the shell writes,
    // because `printf` runs after everything that was sourced.
    let last = text.lines().rfind(|line| line.contains('/'))?;
    let dirs: Vec<PathBuf> = std::env::split_paths(last.trim()).collect();
    (!dirs.is_empty()).then_some(dirs)
}

fn probe_cache() -> &'static Mutex<std::collections::HashMap<String, Option<PathBuf>>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<PathBuf>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Asks an interactive shell what `name` resolves to.
///
/// `command -v` rather than `which`: it is a shell builtin, so it sees aliases
/// and functions, and it is specified by POSIX rather than varying per distro.
/// Its first line is a path for a real command and a description for anything
/// else, which is exactly what the settings pane wants to show.
fn ask_shell(name: &str) -> Option<PathBuf> {
    let script = format!("command -v -- {}", quote(name));
    let first = run_in_shell(script, Vec::new(), "command")?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_owned();

    Some(PathBuf::from(first))
}

/// Runs `script` in an interactive login shell and returns what it printed.
///
/// On a worker with a timeout: a login shell that blocks — on a network mount,
/// on a prompt framework phoning home — must not take the window with it.
/// `env` is set for the shell over ket's own, and `what` names the probe in
/// the warning a timeout logs. `None` for a shell that failed, timed out or
/// could not start, which every caller reads as "the shell said nothing".
fn run_in_shell(script: String, env: Vec<(String, String)>, what: &'static str) -> Option<String> {
    let shell = login_shell();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = std::process::Command::new(&shell)
            .args(["-i", "-c", &script])
            .envs(env)
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(output);
    });

    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(Ok(output)) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(_) => None,
        Err(_) => {
            tracing::warn!(
                probe = what,
                "shell probe timed out; taking that as no answer"
            );
            None
        }
    }
}

/// What the value line printed by [`seen_by_script`] starts with.
///
/// A prompt framework that prints on startup would otherwise be mistaken for
/// the answer; the marker makes the answer the one line that carries it.
const SEEN_BY_MARKER: &str = "KET_SEEN_VALUE=";

fn seen_by_cache() -> &'static Mutex<std::collections::HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<String>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The script behind [`variable_seen_by`]; see there for its shape.
///
/// `case` on what `command -v` says: a first word that resolves to a file is
/// reported without being typed, because a function could not intercept a
/// file anyway and typing it would run it. A newline goes out before the
/// marker so a startup file that printed without one cannot glue itself to
/// the answer.
fn seen_by_script(line: &str, binary: &str, variable: &str) -> String {
    let mut script =
        format!("__ket_seen() {{ printf '\\n%s%s\\n' {SEEN_BY_MARKER} \"${{{variable}-}}\"; }}\n");
    let first = line.split_whitespace().next().unwrap_or_default();
    if first.contains('/') {
        script.push_str("__ket_seen\n");
    } else {
        script.push_str(&format!(
            "case \"$(command -v -- {} 2>/dev/null)\" in\n\
             /*) __ket_seen ;;\n\
             *) function {binary} {{ __ket_seen; }}; {line} ;;\n\
             esac\n",
            quote(first)
        ));
    }
    script
}

/// The answer in a probe's output: the last marked line, when it carries one.
fn seen_by_value(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .find_map(|line| line.strip_prefix(SEEN_BY_MARKER))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Whether `word` can stand in a script, unquoted, as a function name.
fn is_plain_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Whether `name` is a variable a POSIX shell would accept.
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Looks for `binary` in each `PATH` entry, in order.
fn walk_path(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let candidate = PathBuf::from(binary);
        return is_executable(&candidate).then_some(candidate);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
}

/// Whether `path` is a file this user could run.
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The generated `.zshenv`.
///
/// **Order is load-bearing.** Every function is defined above the point where
/// the user's own `.zshenv` is sourced. A user file ending in `emulate sh` puts
/// everything after it under sh parsing rules, and zsh-only syntax then fails
/// to parse — taking the wrapper with it. Function bodies are parsed where they
/// are written, so defining them first makes them immune, and `emulate -L zsh`
/// inside each body restores zsh semantics at call time.
///
/// Only this one file is written. `ZDOTDIR` is handed back on the first lines,
/// so `.zprofile`, `/etc/zshrc`, `.zshrc` and `.zlogin` all come from the
/// user's own directory, and nothing ket wrote is read after this point.
const ZSH_WRAPPER: &str = r#"# ket zsh startup wrapper — generated, do not edit.
#
# ket launches an agent by starting your login shell and typing the command at
# its first prompt, so that your aliases, functions and PATH are all in scope.
# This file exists only to install the hook that types it. It hands ZDOTDIR
# back to your own directory immediately, so every other startup file is read
# exactly as it would be without ket.

__ket_usable_zdotdir() {
  builtin emulate -L zsh
  builtin typeset candidate="$1" file
  [[ -n "$candidate" && -d "$candidate" ]] || return 1
  for file in .zshenv .zshrc .zprofile .zlogin; do
    [[ -e "$candidate/$file" ]] && return 0
  done
  return 1
}

if __ket_usable_zdotdir "${KET_ORIG_ZDOTDIR:-}"; then
  builtin export ZDOTDIR="$KET_ORIG_ZDOTDIR"
else
  builtin unset ZDOTDIR
fi
builtin unset KET_ORIG_ZDOTDIR
builtin unfunction __ket_usable_zdotdir

# The widget that types the line. zle-line-init is the first moment the line
# editor exists; a precmd hook fires before the pty is in line-editing mode and
# would put the command somewhere nothing reads it.
__ket_line_init() {
  if [[ -n "${__ket_prev_line_init:-}" ]]; then
    "${__ket_prev_line_init}" "$@"
  fi
  if (( ${+KET_SHELL_STARTUP_COMMAND} )); then
    BUFFER="$KET_SHELL_STARTUP_COMMAND"
    CURSOR=${#BUFFER}
    builtin unset KET_SHELL_STARTUP_COMMAND
    zle accept-line
  fi
}

# Registered from the first prompt rather than here, so it wins over a widget
# the user's own .zshrc installed. Whatever was there is called first, so a
# prompt framework's line-init still runs.
__ket_deferred_init() {
  builtin emulate -L zsh
  (( $+_ket_deferred_init_done )) && return 0
  builtin typeset -g _ket_deferred_init_done=1
  builtin typeset -g precmd_functions
  precmd_functions=(${precmd_functions:#__ket_deferred_init})

  if [[ "${widgets[zle-line-init]:-}" == "user:__ket_line_init" ]]; then
    :
  elif (( ${+widgets[zle-line-init]} )) && [[ "${widgets[zle-line-init]}" == user:* ]]; then
    builtin typeset -g __ket_prev_line_init="${widgets[zle-line-init]#user:}"
    zle -N zle-line-init __ket_line_init
  else
    builtin typeset -g __ket_prev_line_init=""
    zle -N zle-line-init __ket_line_init
  fi

  builtin unfunction __ket_deferred_init
}

# Why `{ } always { }`: the whole compound command is parsed before any of it
# runs, so the registration below is already parsed as zsh even if the user's
# .zshenv switches the shell into sh emulation. Sourced at top level, not in a
# function, so their exports, options and fpath land in the usual scope.
{
  builtin typeset _ket_user_zshenv="${ZDOTDIR-$HOME}/.zshenv"
  [[ ! -r "$_ket_user_zshenv" ]] || builtin source -- "$_ket_user_zshenv"
} always {
  builtin unset _ket_user_zshenv
  builtin typeset -ag precmd_functions
  (( ${precmd_functions[(Ie)__ket_deferred_init]} )) || precmd_functions+=(__ket_deferred_init)
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_accept_only_shell_safe_function_name_characters() {
        assert!(is_plain_word("claude"));
        assert!(is_plain_word("claude-code"));
        assert!(is_plain_word("claude_code.sh"));
        assert!(!is_plain_word(""));
        assert!(!is_plain_word("claude code"));
        assert!(!is_plain_word("claude;rm"));
        assert!(!is_plain_word("$claude"));
    }

    #[test]
    fn variable_names_follow_posix_shell_rules() {
        assert!(is_variable_name("PATH"));
        assert!(is_variable_name("_private"));
        assert!(is_variable_name("a1b2"));
        assert!(!is_variable_name(""));
        assert!(!is_variable_name("1START"));
        assert!(!is_variable_name("HAS SPACE"));
        assert!(!is_variable_name("HAS-DASH"));
    }

    #[test]
    fn seen_by_value_reads_the_last_marked_line() {
        let text = format!("noise\n{SEEN_BY_MARKER}/first\nmore noise\n{SEEN_BY_MARKER}/second\n");
        assert_eq!(seen_by_value(&text), Some("/second".to_owned()));
    }

    #[test]
    fn seen_by_value_is_none_without_a_marked_line() {
        assert_eq!(seen_by_value("just some output\nno marker here\n"), None);
    }

    #[test]
    fn seen_by_value_is_none_when_the_marked_value_is_empty() {
        let text = format!("{SEEN_BY_MARKER}\n");
        assert_eq!(seen_by_value(&text), None);
    }

    #[test]
    fn seen_by_script_types_the_line_directly_when_the_binary_is_a_path() {
        let script = seen_by_script("/usr/bin/claude hello", "claude", "CLAUDE_CONFIG_DIR");
        assert!(script.contains("__ket_seen\n"));
        assert!(!script.contains("function claude"));
    }

    #[test]
    fn seen_by_script_wraps_a_plain_binary_in_a_function() {
        let script = seen_by_script("claude hello", "claude", "CLAUDE_CONFIG_DIR");
        assert!(script.contains("function claude"));
        assert!(script.contains("claude hello"));
        assert!(script.contains("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn walk_path_with_a_slash_checks_that_exact_file() {
        let dir = std::env::temp_dir().join(format!("ket-shell-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("my-script.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();

        assert_eq!(
            walk_path(script.to_str().unwrap()),
            None,
            "not executable yet"
        );

        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(walk_path(script.to_str().unwrap()), Some(script.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_executable_is_false_for_a_directory_and_a_missing_path() {
        assert!(!is_executable(std::path::Path::new("/definitely/not/here")));
        assert!(!is_executable(std::path::Path::new("/tmp")));
    }
}
