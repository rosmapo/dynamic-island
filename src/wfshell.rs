//! Protokol Wayfire Shell (`zwf_shell_manager_v2`, verzia 3): hotspot na hornom okraji pre autohide.
//! Väzby sa generujú z `protocols/wayfire-shell-unstable-v2.xml` (kópia z Wayfire 0.11).
#![allow(dead_code, non_upper_case_globals, non_camel_case_types, unused_imports, clippy::all)]

pub mod proto {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/wayfire-shell-unstable-v2.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/wayfire-shell-unstable-v2.xml");
}
