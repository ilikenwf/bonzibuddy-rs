//! Test utility: publish `_NET_WORKAREA` on the root window the way an EWMH
//! window manager does, so the pet's work-area path can be exercised without
//! a full desktop (some minimal hosts lack a working `xprop -set`).
//!
//! Usage: `cargo run --example set_workarea -- X Y W H [X Y W H ...]`
//! then start the pet and watch startup placement, walk bounce, and the
//! PropertyNotify reseat. Plain Xvfb drops root properties when the setting
//! client disconnects, so `SET_WORKAREA_HOLD=secs` keeps the connection —
//! and the property — alive while you start the pet.
//!
//! With no values it reads the property back instead:
//! `cargo run --example set_workarea` → `read back: …`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    let vals: Vec<u32> = std::env::args()
        .skip(1)
        .map(|s| s.parse::<u32>())
        .collect::<Result<_, _>>()?;
    if !vals.is_empty() && vals.len() % 4 != 0 {
        return Err("need multiples of four values: X Y W H per monitor".into());
    }
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = conn.intern_atom(false, b"_NET_WORKAREA")?.reply()?.atom;
    if vals.is_empty() {
        let r = conn.get_property(false, root, atom, x11rb::protocol::xproto::AtomEnum::CARDINAL, 0, 1024)?.reply()?;
        println!("read back: type={:?} format={} len={}", r.type_, r.format, r.value.len());
        return Ok(());
    }
    let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_ne_bytes()).collect();
    conn.change_property(
        x11rb::protocol::xproto::PropMode::REPLACE,
        root,
        atom,
        x11rb::protocol::xproto::AtomEnum::CARDINAL,
        32,
        vals.len() as u32,
        &bytes,
    )?;
    conn.flush()?;
    eprintln!("set _NET_WORKAREA = {vals:?}");
    if let Ok(secs) = std::env::var("SET_WORKAREA_HOLD") {
        std::thread::sleep(std::time::Duration::from_secs(secs.parse()?));
    }
    Ok(())
}
