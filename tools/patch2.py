import io, os

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(BASE)


def rd(p):
    return io.open(p, encoding="utf8", newline="").read()


def wr(p, s):
    io.open(p, "w", encoding="utf8", newline="").write(s)


# --- single_instance.rs: drop the unused instance_running helper ---
# Historical one-off: the helper is already gone from the source, so this is a no-op now.
p = "gm-gui/src/single_instance.rs"
s = rd(p)
helper = (
    "/// True when another instance already serves the control pipe.\n"
    "pub fn instance_running() -> bool {\n    ipc::is_instance_running()\n}\n\n"
)
t = s.replace(helper, "", 1).replace(helper.replace("\n", "\r\n"), "", 1)
if t != s:
    wr(p, t)
    print("single_instance cleaned")
else:
    print("single_instance: helper not present, nothing to do")
