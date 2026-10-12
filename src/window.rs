// Window managers like xfwm4 ignore the position gpui requests when creating
// the window, so it's moved once it is mapped.
#[cfg(target_os = "linux")]
pub fn center_window() {
    // Wayland clients cannot position their own windows.
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return;
    }
    if let Err(error) = center_x11_window() {
        eprintln!("failed to center window: {error}");
    }
}

#[cfg(not(target_os = "linux"))]
pub fn center_window() {}

#[cfg(target_os = "linux")]
fn center_x11_window() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt};

    let (connection, screen_index) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen_index].root;
    // Create atoms that don't exist yet: with `only_if_exists` a missing atom
    // comes back as 0, and reading a property with atom 0 fails (BadAtom)
    // instead of reading as missing.
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(connection.intern_atom(false, name)?.reply()?.atom)
    };

    // gpui doesn't expose the X11 window id, so find our window via the window
    // manager's client list.
    let clients = connection
        .get_property(
            false,
            root,
            atom(b"_NET_CLIENT_LIST")?,
            AtomEnum::WINDOW,
            0,
            u32::MAX,
        )?
        .reply()?;
    let net_wm_pid = atom(b"_NET_WM_PID")?;
    let pid = std::process::id();
    let window_id = clients
        .value32()
        .into_iter()
        .flatten()
        .find(|&client| {
            connection
                .get_property(false, client, net_wm_pid, AtomEnum::CARDINAL, 0, 1)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|reply| reply.value32()?.next())
                == Some(pid)
        })
        .ok_or("window not found in _NET_CLIENT_LIST")?;

    let workarea: Option<Vec<u32>> = connection
        .get_property(
            false,
            root,
            atom(b"_NET_WORKAREA")?,
            AtomEnum::CARDINAL,
            0,
            4,
        )?
        .reply()?
        .value32()
        .map(Iterator::collect)
        .filter(|values: &Vec<u32>| values.len() == 4);
    let [area_x, area_y, area_width, area_height] = match workarea {
        Some(values) => [values[0], values[1], values[2], values[3]].map(|value| value as i32),
        None => {
            let root = connection.get_geometry(root)?.reply()?;
            [0, 0, root.width as i32, root.height as i32]
        }
    };

    // [left, right, top, bottom]; a missing property means no extents.
    let extents = |name: &[u8]| -> Result<[i32; 4], Box<dyn std::error::Error>> {
        Ok(connection
            .get_property(false, window_id, atom(name)?, AtomEnum::CARDINAL, 0, 4)?
            .reply()?
            .value32()
            .map(|values| values.collect::<Vec<_>>())
            .and_then(|values| <[u32; 4]>::try_from(values).ok())
            .unwrap_or_default()
            .map(|value| value as i32))
    };

    // The window manager positions the outer frame, so center the window
    // including its decorations.
    let [left, right, top, bottom] = extents(b"_NET_FRAME_EXTENTS")?;
    // With client-side decorations the transparent shadow area may hang off
    // the work area; the visible edge may not.
    let [shadow_left, _, shadow_top, _] = extents(b"_GTK_FRAME_EXTENTS")?;

    let geometry = connection.get_geometry(window_id)?.reply()?;
    let outer_width = geometry.width as i32 + left + right;
    let outer_height = geometry.height as i32 + top + bottom;

    let x = (area_x + (area_width - outer_width) / 2).max(area_x - shadow_left);
    // Optical centering: slightly less space above the window than below.
    let y = (area_y + (area_height - outer_height) * 2 / 5).max(area_y - shadow_top);
    connection.configure_window(window_id, &ConfigureWindowAux::new().x(x).y(y))?;
    connection.flush()?;

    // Wait (briefly) for the window manager to apply the move, so a following
    // maximize records the centered position as the one to restore to.
    let target = ((x + left) as i16, (y + top) as i16);
    for _ in 0..50 {
        let position = connection
            .translate_coordinates(window_id, root, 0, 0)?
            .reply()?;
        if (position.dst_x, position.dst_y) == target {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}
