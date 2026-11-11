import io, os

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(BASE)

p = "gm-gui/src/single_instance.rs"
s = io.open(p, encoding="utf8").read()

if "fn verb_from_args" not in s:
    add = '''
/// Parses the first recognised switch from the arguments (program name already removed).
pub fn verb_from_args(args: &[String]) -> Option<Verb> {
    for a in args {
        let t = a.trim_start_matches('-').trim_start_matches('/').to_ascii_lowercase();
        match t.as_str() {
            "on" | "enable" | "start" => return Some(Verb::Enable),
            "off" | "disable" | "stop" => return Some(Verb::Disable),
            "toggle" => return Some(Verb::Toggle),
            "show" | "open" | "panel" => return Some(Verb::Show),
            "quit" | "exit" => return Some(Verb::Quit),
            "status" => return Some(Verb::Status),
            _ => {}
        }
    }
    None
}
'''
    s = s.rstrip() + "\n" + add
    io.open(p, "w", encoding="utf8", newline="").write(s)
    print("patched single_instance.rs")
else:
    print("already patched")