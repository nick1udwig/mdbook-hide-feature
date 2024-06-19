use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Arg, Command};
use log::{debug, LevelFilter, SetLoggerError};
use log::{Level, Metadata, Record};
use mdbook::book::BookItem;
use mdbook::preprocess::CmdPreprocessor;
use regex::{CaptureMatches, Captures, Regex};

static LOGGER: SimpleLogger = SimpleLogger;

pub fn init() -> Result<(), SetLoggerError> {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info))
}

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{} - {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn make_app() -> Command {
    Command::new("hide-feature-preprocessor")
        .about("A mdbook preprocessor which does stuff")
        .subcommand(
            Command::new("supports")
                .arg(Arg::new("renderer").required(true))
                .about("Check whether a renderer is supported by this preprocessor"),
        )
}

/// Filters out blocks of code that are enclosed in #[cfg(feature = "feature_name")]
fn filter_features(contents: &str, feature_name: &str) -> String {
    let mut result = String::new();
    let mut skip = false;
    let mut brace_count = 0;

    let re_start = regex::Regex::new(&format!(
        r#"^\s*#\s*\[cfg\(feature = "{}"\)\]"#,
        feature_name
    ))
    .unwrap();
    let re_open_brace = regex::Regex::new(r"\{").unwrap();
    let re_close_brace = regex::Regex::new(r"\}").unwrap();

    for line in contents.lines() {
        if skip {
            // Count braces only if we are inside a skipped section
            if re_open_brace.is_match(line) {
                brace_count += line.matches('{').count();
            }
            if re_close_brace.is_match(line) {
                brace_count -= line.matches('}').count();
            }

            // Continue skipping until all opened braces are closed
            if brace_count == 0 {
                skip = false;
            }

            result.push_str(&format!("# {line}"));
            result.push('\n');
            continue;
        }

        // Check if the line contains the start of a cfg feature block
        if re_start.is_match(line) {
            skip = true;

            result.push_str(&format!("# {line}"));
            result.push('\n');
            continue;
        }

        // Add the line to the result if not skipping
        result.push_str(line);
        result.push('\n');
    }

    result
}

pub fn replace_all<P: AsRef<Path>>(s: &str, path: P) -> Result<String> {
    // When replacing one thing in a string by something with a different length,
    // the indices after that will not correspond,
    // we therefore have to store the difference to correct this
    let mut previous_end_index = 0;
    let mut replaced = String::new();

    for playpen in find_links(s) {
        replaced.push_str(&s[previous_end_index..playpen.start_index]);
        replaced.push_str(&playpen.render_with_path(&path)?);
        previous_end_index = playpen.end_index;
    }

    replaced.push_str(&s[previous_end_index..]);
    Ok(replaced)
}

#[derive(PartialOrd, PartialEq, Debug, Clone)]
enum LinkType {
    IncludeHideTest(PathBuf),
}

#[derive(PartialOrd, PartialEq, Debug, Clone)]
struct Link {
    start_index: usize,
    end_index: usize,
    link: LinkType,
    link_text: String,
}

impl Link {
    fn from_capture(cap: Captures) -> Option<Link> {
        let link_type = match (cap.get(0), cap.get(1), cap.get(2)) {
            (_, Some(typ), Some(rest)) => {
                let mut path_props = rest.as_str().split_whitespace();
                let file_path = path_props.next().map(PathBuf::from);

                match (typ.as_str(), file_path) {
                    ("includehidetest", Some(pth)) => Some(LinkType::IncludeHideTest(pth)),
                    _ => None,
                }
            }
            _ => None,
        };

        link_type.and_then(|lnk| {
            cap.get(0).map(|mat| Link {
                start_index: mat.start(),
                end_index: mat.end(),
                link: lnk,
                link_text: mat.as_str().to_string(),
            })
        })
    }

    fn render_with_path<P: AsRef<Path>>(&self, base: P) -> Result<String> {
        let base = base.as_ref();
        match self.link {
            // omit the escape char
            LinkType::IncludeHideTest(ref pat) => {
                // get file
                let contents = std::fs::read_to_string(base.join(pat))?;
                // run regex above on it
                let contents = filter_features(&contents, "test");
                // give result
                Ok(contents)
                //file_to_string(base.join(pat)).chain_err(|| format!("Could not read file for link {}", self.link_text))
            }
        }
    }
}

struct LinkIter<'a>(CaptureMatches<'a, 'a>);

impl<'a> Iterator for LinkIter<'a> {
    type Item = Link;
    fn next(&mut self) -> Option<Link> {
        for cap in &mut self.0 {
            if let Some(inc) = Link::from_capture(cap) {
                return Some(inc);
            }
        }
        None
    }
}

fn find_links(contents: &str) -> LinkIter {
    // lazily compute following regex
    // r"\\\{\{#.*\}\}|\{\{#([a-zA-Z0-9]+)\s*([a-zA-Z0-9_.\-:/\\\s]+)\}\}")?;
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"(?x) # insignificant whitespace mode
                    \\\{\{\#.*\}\}               # match escaped link
                    |                            # or
                    \{\{\s*                      # link opening parens and whitespace
                      \#([a-zA-Z0-9]+)           # link type
                      \s+                        # separating whitespace
                      ([a-zA-Z0-9\s_.\-:/\\]+)   # link target path and space separated properties
                    \s*\}\}                      # whitespace and link closing parens
                                 ").unwrap();
    }
    LinkIter(RE.captures_iter(contents))
}

fn main() {
    init().unwrap();
    let matches = make_app().get_matches();
    if let Some(_sub_args) = matches.subcommand_matches("supports") {
        std::process::exit(0);
    }

    let (_ctx, mut book) = CmdPreprocessor::parse_input(io::stdin()).unwrap();
    book.for_each_mut(|item| match item {
        BookItem::Chapter(ref mut chapter) => {
            let old = chapter.content.clone();
            chapter.content = replace_all(
                &chapter.content,
                PathBuf::from("src").join(chapter.path.as_ref().and_then(|p| p.parent()).unwrap()),
            )
            .unwrap();
            if chapter.content != old {
                debug!("old:{}\nnew:{}", old, chapter.content);
            }
        }
        _ => {}
    });

    serde_json::to_writer(io::stdout(), &book).unwrap();
}
