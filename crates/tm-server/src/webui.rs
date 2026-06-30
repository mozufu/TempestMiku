use std::path::PathBuf;

use tower_http::{
    services::{ServeDir, ServeFile},
    set_status::SetStatus,
};

pub fn flutter_web_root() -> PathBuf {
    std::env::var_os("TM_WEBUI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("clients")
                .join("miku_flutter")
                .join("build")
                .join("web")
        })
}

pub fn service() -> ServeDir<SetStatus<ServeFile>> {
    let root = flutter_web_root();
    ServeDir::new(&root).not_found_service(ServeFile::new(root.join("index.html")))
}
