use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::prelude::PermissionsExt,
    path::{PathBuf},
};

use zstd::{stream::AutoFinishEncoder, Encoder};

pub fn create_zstd_writer(
    path: PathBuf,
) -> Result<AutoFinishEncoder<'static, File>, std::io::Error> {
    let f = create_plain_writer(path)?;

    let encoder = Encoder::new(f, 3)?;

    Ok(encoder.auto_finish())
}

pub fn create_plain_writer(path: PathBuf) -> Result<File, std::io::Error> {
    if !path.parent().expect("Failed to get parent").exists() {
        std::fs::create_dir_all(path.parent().expect("Failed to get parent"))?;
    }

    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .open(path.to_owned())?;

    let metadata = f.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o777);
    assert_eq!(permissions.mode(), 0o777);

    f.set_permissions(permissions)?;

    Ok(f)
}