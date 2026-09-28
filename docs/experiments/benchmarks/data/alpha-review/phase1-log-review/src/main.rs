use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

mod alpha {
    pub mod journal {
        #[derive(Clone)]
        pub struct Cursor {
            pub path: String,
            pub device: u64,
            pub inode: u64,
            pub offset: u64,
            pub skipping_oversize: bool,
        }
    }
    #[path = "/home/kmosoti/projects/fabric_o11y/src/alpha/log_source.rs"]
    pub mod log_source;
}

fn run(mode: &str, path: &Path) -> io::Result<bool> {
    use alpha::log_source::read_lines;
    match mode {
        "valid" => {
            fs::write(path, b"ok\npartial")?;
            let first = read_lines(path, None, 100)?;
            let first_ok = first.lines.len() == 1 && first.lines[0].body == "ok"
                && first.cursor.offset == 3 && first.gaps.is_empty();
            OpenOptions::new().append(true).open(path)?.write_all(b"\n")?;
            let second = read_lines(path, Some(&first.cursor), 100)?;
            let second_ok = second.lines.len() == 1 && second.lines[0].body == "partial"
                && second.cursor.offset == 11 && second.gaps.is_empty();
            println!("valid first_cursor={} second_cursor={} first_ok={} second_ok={}",
                     first.cursor.offset, second.cursor.offset, first_ok, second_ok);
            Ok(first_ok && second_ok)
        }
        "oversize" => {
            let mut input = vec![b'X'; 256 * 1024 + 10];
            input.extend_from_slice(b"\nnormal\n");
            fs::write(path, &input)?;
            let mut cursor = None;
            let mut saw_normal = false;
            let mut only_normal = true;
            let mut saw_skip = false;
            for attempt in 1..=3 {
                let out = read_lines(path, cursor.as_ref(), 100)?;
                saw_normal |= out.lines.iter().any(|line| line.body == "normal");
                only_normal &= out.lines.iter().all(|line| line.body == "normal");
                saw_skip |= out.cursor.skipping_oversize;
                println!("oversize attempt={} cursor={} skipping={} lines={:?} gaps={:?}",
                         attempt, out.cursor.offset, out.cursor.skipping_oversize,
                         out.lines.iter().map(|line| line.body.as_str()).collect::<Vec<_>>(), out.gaps);
                // Reconstruct the cursor as though it were loaded from the
                // journal before the next collection pass.
                cursor = Some(alpha::journal::Cursor {
                    path: out.cursor.path, device: out.cursor.device,
                    inode: out.cursor.inode, offset: out.cursor.offset,
                    skipping_oversize: out.cursor.skipping_oversize,
                });
            }
            Ok(saw_skip && saw_normal && only_normal
               && cursor.unwrap().offset == input.len() as u64)
        }
        "fifo" => match read_lines(path, None, 100) {
            Err(error) => {
                println!("fifo error_kind={:?} error={error}", error.kind());
                Ok(error.kind() == io::ErrorKind::InvalidInput)
            }
            Ok(_) => Ok(false),
        },
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "unknown mode")),
    }
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode");
    let path = args.next().expect("path");
    if !run(&mode, Path::new(&path))? {
        std::process::exit(1);
    }
    Ok(())
}
