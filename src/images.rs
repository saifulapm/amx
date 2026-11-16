//! Images handed to an agent with its task or a message.
//!
//! Each image is checked before anything is started, then copied into the
//! agent's record as `images/<n>.<ext>`, so it outlives the file it came from
//! and goes when the record does. The vendor is given the copy's path in the
//! way its table entry names ([`crate::vendor::Attach`]).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::vendor::Attach;

/// The largest image amx hands on. Below it the vendors resize.
pub const LIMIT: u64 = 20 * MB;

const MB: u64 = 1024 * 1024;

/// The image types amx attaches, by the first bytes of the file, with the
/// extension a copy is saved under.
pub fn kind(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [0xff, 0xd8, 0xff, ..] => Some("jpg"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("webp"),
        _ => None,
    }
}

/// The refusal for an image of `bytes`, or `None` when it is small enough.
///
/// Rounded up, so an image a byte over the limit does not read as 20 MB.
pub fn too_big(bytes: u64) -> Option<String> {
    (bytes > LIMIT).then(|| {
        format!(
            "the image is {} MB; the limit is {} MB",
            bytes.div_ceil(MB),
            LIMIT / MB
        )
    })
}

/// Checks a file named with `--image`, answering its extension or why it
/// cannot be attached. `path` is named in the refusal as it was given.
pub fn checked(path: &Path) -> Result<&'static str, String> {
    let given = path.display();
    let Ok(size) = std::fs::metadata(path).map(|meta| meta.len()) else {
        return Err(format!("no such image: {given}"));
    };
    if let Some(refusal) = too_big(size) {
        return Err(format!("{given}: {refusal}"));
    }
    let mut head = [0u8; 12];
    let read = std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read(&mut file, &mut head))
        .map_err(|e| format!("{given}: {e}"))?;
    kind(&head[..read]).ok_or_else(|| format!("{given} is not a png, jpeg, webp or gif image"))
}

/// Copies `images` into `agent_dir/images`, numbered on from the last image
/// already there, and answers the copies' paths in order.
///
/// Numbers run per record, not per message, so a second send never writes
/// over the first.
pub fn filed(agent_dir: &Path, images: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if images.is_empty() {
        return Ok(Vec::new());
    }
    let dir = agent_dir.join("images");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut last = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let stem = Path::new(&name).file_stem()?.to_str()?.to_string();
            stem.parse::<u64>().ok()
        })
        .max()
        .unwrap_or(0);
    images
        .iter()
        .map(|image| {
            let ext = checked(image).map_err(anyhow::Error::msg)?;
            last += 1;
            let copy = dir.join(format!("{last}.{ext}"));
            std::fs::copy(image, &copy)
                .with_context(|| format!("copying {} to {}", image.display(), copy.display()))?;
            Ok(copy)
        })
        .collect()
}

/// `message` with `images` attached the way `attach` says.
///
/// [`Attach::Argument`] and [`Attach::Flag`] put the images in argv words of
/// their own ([`arguments`], [`flagged`]), and [`Attach::Pasted`] in pastes
/// of their own, so the message is answered as it is.
pub fn attached(attach: Attach, images: &[PathBuf], message: &str) -> String {
    let with = |each: fn(&Path) -> String, between: &str| {
        images
            .iter()
            .map(|image| each(image))
            .chain((!message.is_empty()).then(|| message.to_string()))
            .collect::<Vec<_>>()
            .join(between)
    };
    match attach {
        Attach::Mentioned => with(mention, " "),
        Attach::Lines => with(|image| format!("image: {}", image.display()), "\n"),
        Attach::Argument | Attach::Flag(_) | Attach::Pasted { .. } => message.to_string(),
    }
}

/// One `<flag>=<path>` argv word per image.
pub fn flagged(flag: &str, images: &[PathBuf]) -> Vec<String> {
    images
        .iter()
        .map(|image| format!("{flag}={}", image.display()))
        .collect()
}

/// One `@<path>` argv word per image, unquoted: the word is the path whole.
pub fn arguments(images: &[PathBuf]) -> Vec<String> {
    images
        .iter()
        .map(|image| format!("@{}", image.display()))
        .collect()
}

/// An `@` word for `image`, quoted when its path holds whitespace so the
/// vendor reads it as one path.
fn mention(image: &Path) -> String {
    let path = image.display().to_string();
    match path.contains(char::is_whitespace) {
        true => format!("@\"{path}\""),
        false => format!("@{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_reads_the_four_types_by_their_first_bytes() {
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n"), Some("png"));
        assert_eq!(kind(b"\xff\xd8\xff\xe0"), Some("jpg"));
        assert_eq!(kind(b"GIF89a"), Some("gif"));
        assert_eq!(kind(b"RIFF\x10\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(kind(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(kind(b"hello"), None);
    }

    #[test]
    fn an_image_over_twenty_mb_is_refused_by_its_size() {
        assert_eq!(too_big(LIMIT), None);
        assert_eq!(
            too_big(21 * MB).as_deref(),
            Some("the image is 21 MB; the limit is 20 MB")
        );
        assert_eq!(
            too_big(LIMIT + 1).as_deref(),
            Some("the image is 21 MB; the limit is 20 MB")
        );
    }

    #[test]
    fn checked_names_the_file_as_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        let big = dir.path().join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(21 * MB).unwrap();

        assert_eq!(
            checked(Path::new("missing.png")),
            Err("no such image: missing.png".to_string())
        );
        assert_eq!(
            checked(&big),
            Err(format!(
                "{}: the image is 21 MB; the limit is 20 MB",
                big.display()
            ))
        );
        assert_eq!(
            checked(&text),
            Err(format!(
                "{} is not a png, jpeg, webp or gif image",
                text.display()
            ))
        );
    }

    #[test]
    fn filed_numbers_on_from_the_images_the_record_already_holds() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("shot.png");
        std::fs::write(&source, b"\x89PNG\r\n\x1a\nshot").unwrap();
        let record = dir.path().join("record");
        std::fs::create_dir_all(record.join("images")).unwrap();
        std::fs::write(record.join("images/1.jpg"), b"\xff\xd8\xffold").unwrap();

        let copies = filed(&record, &[source.clone(), source]).unwrap();

        assert_eq!(
            copies,
            [record.join("images/2.png"), record.join("images/3.png")]
        );
        assert_eq!(std::fs::read(&copies[0]).unwrap(), b"\x89PNG\r\n\x1a\nshot");
    }

    #[test]
    fn a_mention_is_quoted_only_where_the_path_holds_a_space() {
        let images = [
            PathBuf::from("/state/agents/a1/images/1.png"),
            PathBuf::from("/my state/agents/a1/images/2.png"),
        ];
        assert_eq!(
            attached(Attach::Mentioned, &images, "what is this"),
            "@/state/agents/a1/images/1.png @\"/my state/agents/a1/images/2.png\" what is this"
        );
        assert_eq!(
            attached(Attach::Mentioned, &images[..1], ""),
            "@/state/agents/a1/images/1.png"
        );
        assert_eq!(attached(Attach::Mentioned, &[], "x"), "x");
    }

    #[test]
    fn image_lines_go_one_per_image_above_the_message() {
        let images = [
            PathBuf::from("/state/agents/a1/images/1.png"),
            PathBuf::from("/my state/agents/a1/images/2.png"),
        ];
        assert_eq!(
            attached(Attach::Lines, &images, "describe"),
            "image: /state/agents/a1/images/1.png\n\
             image: /my state/agents/a1/images/2.png\n\
             describe"
        );
        assert_eq!(
            attached(Attach::Lines, &images[..1], ""),
            "image: /state/agents/a1/images/1.png"
        );
    }

    #[test]
    fn an_argument_is_one_unquoted_word_per_image_and_leaves_the_message() {
        let images = [
            PathBuf::from("/state/agents/a1/images/1.png"),
            PathBuf::from("/my state/agents/a1/images/2.png"),
        ];
        assert_eq!(
            arguments(&images),
            [
                "@/state/agents/a1/images/1.png",
                "@/my state/agents/a1/images/2.png"
            ]
        );
        assert_eq!(attached(Attach::Argument, &images, "x"), "x");
    }
}
