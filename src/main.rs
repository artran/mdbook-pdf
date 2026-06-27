/**
 * mdbook-pdf
 * A PDF generator for mdBook using headless Chrome.
 *
 * Author:  Hollow Man <hollowman@opensuse.org>
 * License: GPL-3.0
 *
 * Copyright (C) 2022-2025 Hollow Man (@HollowMan6)
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */
use headless_chrome::{Browser, LaunchOptionsBuilder, types::PrintToPdfOptions};
use lazy_static::lazy_static;
use mdbook_core::book::BookItem::Chapter;
use mdbook_renderer::RenderContext;
use regex::Regex;
use std::io::{BufReader, BufWriter, Read, Write};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

lazy_static! {
    static ref SCHEME_LINK: Regex = Regex::new(r"^[a-z][a-z0-9+.-]*:").unwrap();
    static ref A_LINK: Regex = Regex::new(r#"(<a [^>]*?href=")([^"]+?)""#).unwrap();
    static ref PATH_TO_ROOT: Regex =
        Regex::new(r#"const\s+path_to_root\s*=\s*\"([^\"]*)\""#).unwrap();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Receives the data passed to the program via mdbook
    let mut stdin = io::stdin();

    // Get the configs
    let ctx = RenderContext::from_json(&mut stdin).unwrap();
    let cfg = &ctx.config.outputs::<PrintOptions>()?["pdf"];

    let print_html_path = ctx
        .destination
        .parent()
        .unwrap()
        .join("html")
        .join("print.html");

    if !print_html_path.exists() {
        println!(
            "PDF generation failed. The print.html file does not exist at {}.",
            print_html_path.display()
        );
        println!(
            "Verify output.html is active and output.html.print.enabled is set to true in your book.toml."
        );
        return Err(Box::<io::Error>::from(io::Error::new(
            io::ErrorKind::NotFound,
            format!("File not found: {}", print_html_path.display()),
        )));
    }

    prepare_html_for_pdf(&print_html_path, cfg, &build_toc_fix(&ctx.book))?;

    println!("Generating PDF, please be patient...");
    let generated_pdf_path = ctx.destination.join("output.pdf");
    print_html_to_pdf(&print_html_path, &generated_pdf_path, cfg)?;
    println!(
        "PDF successfully generated at: {}",
        generated_pdf_path.display()
    );

    if cfg.per_chapter {
        println!("Generating chapter PDFs, please be patient...");
        for (chapter_index, chapter) in
            select_chapters_for_pdf(&ctx.book, &cfg.omit_chapters, &cfg.print_chapters)
                .into_iter()
                .enumerate()
        {
            let chapter_html_path =
                chapter_html_path(&ctx.destination, chapter).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "Unable to determine HTML path for chapter: {}",
                            chapter.name
                        ),
                    )
                })?;

            if !chapter_html_path.exists() {
                return Err(Box::<io::Error>::from(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("File not found: {}", chapter_html_path.display()),
                )));
            }

            prepare_html_for_pdf(&chapter_html_path, cfg, "")?;
            let generated_pdf_path =
                chapter_pdf_output_path(&ctx.destination, chapter, chapter_index);
            print_html_to_pdf(&chapter_html_path, &generated_pdf_path, cfg)?;
            println!(
                "Chapter PDF successfully generated at: {}",
                generated_pdf_path.display()
            );
        }
    }

    Ok(())
}

#[macro_use]
extern crate serde_derive;

/**
 * Refer to https://docs.rs/headless_chrome/latest/headless_chrome/protocol/page/struct.PrintToPdfOptions.html
 * for member types of the PrintToPdfOptions
 */

#[derive(Deserialize, Debug, Clone)]
#[serde(default, rename_all = "kebab-case")]
pub struct PrintOptions {
    pub trying_times: u64,
    pub timeout: u64,
    pub browser_binary_path: String,
    pub static_site_url: String,
    pub per_chapter: bool,
    pub omit_chapters: Vec<String>,
    pub print_chapters: Vec<String>,
    pub landscape: bool,
    pub display_header_footer: bool,
    pub print_background: bool,
    pub theme: String,
    pub scale: f64,
    pub paper_width: f64,
    pub paper_height: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub page_ranges: String,
    pub ignore_invalid_page_ranges: bool,
    pub header_template: String,
    pub footer_template: String,
    pub prefer_css_page_size: bool,
    pub generate_document_outline: bool,
    pub generate_tagged_pdf: bool,
}

/**
 * Refer to https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-printToPDF
 * for the default values and meanings of the params for Page.PrintToPDF
 */
impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            trying_times: 1u64,
            timeout: 600u64,
            browser_binary_path: "".to_string(),
            static_site_url: "".to_string(),
            per_chapter: false,
            omit_chapters: Vec::new(),
            print_chapters: Vec::new(),
            landscape: false,
            display_header_footer: false,
            print_background: false,
            theme: "".to_string(),
            scale: 1_f64,
            paper_width: 8.5_f64,
            paper_height: 11_f64,
            margin_top: 1_f64,
            margin_bottom: 1_f64,
            margin_left: 1_f64,
            margin_right: 1_f64,
            page_ranges: "".to_string(),
            ignore_invalid_page_ranges: false,
            header_template: "".to_string(),
            footer_template: "".to_string(),
            prefer_css_page_size: false,
            generate_document_outline: true,
            generate_tagged_pdf: true,
        }
    }
}

fn build_toc_fix(book: &mdbook_core::book::Book) -> String {
    // Insert a link to the page div in the print.html to make sure that generated pdf
    // contains the destination for ToC to locate the specific page in pdf.
    let mut toc_fix = "<div style=\"display: none\">".to_owned();
    for item in book.iter() {
        if let Chapter(chapter) = item {
            let path = chapter.path.clone();
            if let Some(path) = path {
                let print_page_id = {
                    let mut base = path.display().to_string();
                    if base.ends_with(".md") {
                        base.truncate(base.len() - 3);
                    }
                    base.replace(['/', '\\'], "-").to_ascii_lowercase()
                };
                toc_fix.push_str(&(format!(r##"<a href="#{print_page_id}">{print_page_id}</a>"##)));
            }
        }
    }
    toc_fix.push_str("</div>");
    toc_fix
}

fn prepare_html_for_pdf(
    html_path: &Path,
    cfg: &PrintOptions,
    extra_html: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Modify the HTML for custom JS scripts as well as links outside the book.
    let file = fs::OpenOptions::new().read(true).open(html_path)?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;

    let path_to_root = html_path_to_root(&contents).unwrap_or_default();

    contents = contents.replacen("</script>", &(printing_script() + extra_html), 1);
    if !cfg.static_site_url.is_empty() {
        contents = A_LINK
            .replace_all(&contents, |caps: &regex::Captures<'_>| {
                // Ensure that there is no '\' in the link to ensure the following judgement work
                let link = caps[2].replace('\\', "/");
                // Don't modify links with schemes like `https`, and no need to modify pages inside the book.
                if !link.starts_with('#')
                    && !SCHEME_LINK.is_match(&link)
                    && link_escapes_book_root(&link, &path_to_root)
                {
                    let mut fixed_link = String::new();

                    fixed_link.push_str(&cfg.static_site_url);
                    if !fixed_link.ends_with('/') {
                        fixed_link.push('/');
                    }
                    fixed_link.push_str(&link);

                    return format!("{}{}\"", &caps[1], &fixed_link);
                }
                // Otherwise, leave it as-is.
                format!("{}{}\"", &caps[1], &caps[2])
            })
            .into_owned();
    }

    let file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(html_path)?;
    let mut buf_writer = BufWriter::new(file);
    buf_writer.write_all(contents.as_bytes())?;
    Ok(())
}

fn html_path_to_root(contents: &str) -> Option<String> {
    PATH_TO_ROOT
        .captures(contents)
        .and_then(|captures| captures.get(1))
        .map(|path_to_root| path_to_root.as_str().to_string())
}

fn link_escapes_book_root(link: &str, path_to_root: &str) -> bool {
    let mut depth = path_to_root
        .split('/')
        .filter(|component| *component == "..")
        .count();
    let link_path = link.split(['?', '#']).next().unwrap_or(link);

    for component in link_path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            _ => depth += 1,
        }
    }

    false
}

fn printing_script() -> String {
    "</script>

        <!-- Custom JS scripts for mdbook-pdf PDF generation -->
        <script type='text/javascript'>
            let markAllContentHasLoadedForPrinting = () =>
                window.setTimeout(
                    () => {
                        let p = document.createElement('div');
                        p.setAttribute('id', 'content-has-all-loaded-for-mdbook-pdf-generation');
                        document.body.appendChild(p);
                    }, 100
                );

            window.addEventListener('load', () => {
                // Expand all the <details> elements for printing.
                r = document.getElementsByTagName('details');
                for (let i of r)
                    i.open = true;

                try {
                    MathJax.Hub.Register.StartupHook('End', markAllContentHasLoadedForPrinting);
                } catch (e) {
                    markAllContentHasLoadedForPrinting();
                }
            });
        </script>
    "
    .to_owned()
}

fn print_html_to_pdf(
    html_path: &Path,
    generated_pdf_path: &Path,
    cfg: &PrintOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // Used to relieve errors related to timeouts, but now for
    // the patches in my fork, it's not likely to be needed
    let trying_times = cfg.trying_times + 1;

    for time in 1..trying_times {
        let url = format!("file://{}", html_path.display());

        let cloned_cfg = cfg.clone();
        let browser_binary = if cloned_cfg.browser_binary_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(cloned_cfg.browser_binary_path))
        };

        let launch_opts = LaunchOptionsBuilder::default()
            .headless(true)
            .sandbox(false)
            .devtools(false)
            .idle_browser_timeout(Duration::from_secs(cfg.timeout))
            .path(browser_binary)
            .args(vec![
                OsStr::new("--unlimited-storage"),
                OsStr::new("--webkit-print-color-adjust"),
            ])
            .build()?;

        let pdf_opts = PrintToPdfOptions {
            landscape: Some(cloned_cfg.landscape),
            display_header_footer: Some(cloned_cfg.display_header_footer),
            print_background: Some(cloned_cfg.print_background),
            scale: Some(cloned_cfg.scale),
            paper_width: Some(cloned_cfg.paper_width),
            paper_height: Some(cloned_cfg.paper_height),
            margin_top: Some(cloned_cfg.margin_top),
            margin_bottom: Some(cloned_cfg.margin_bottom),
            margin_left: Some(cloned_cfg.margin_left),
            margin_right: Some(cloned_cfg.margin_right),
            page_ranges: Some(cloned_cfg.page_ranges),
            ignore_invalid_page_ranges: Some(cloned_cfg.ignore_invalid_page_ranges),
            header_template: Some(cloned_cfg.header_template),
            footer_template: Some(cloned_cfg.footer_template),
            prefer_css_page_size: Some(cloned_cfg.prefer_css_page_size),
            generate_document_outline: Some(cloned_cfg.generate_document_outline),
            generate_tagged_pdf: Some(cloned_cfg.generate_tagged_pdf),
            transfer_mode: None,
        };

        // Create a new browser window.
        let browser = Browser::new(launch_opts)?;
        let tab = browser.new_tab()?;
        tab.set_default_timeout(std::time::Duration::from_secs(cfg.timeout));
        let page = tab.navigate_to(&url)?.wait_until_navigated()?;
        page.wait_for_element("#content-has-all-loaded-for-mdbook-pdf-generation")?;

        // Accept the Google Analytics cookie.
        if page.find_element("a.cookieBarConsentButton").is_ok() {
            page.evaluate(
                "document.querySelector('a.cookieBarConsentButton').click()",
                false,
            )?;
            println!(
                "The book you built uses cookies from Google to deliver and enhance the quality of its services and to analyze traffic."
            );
            println!("Learn more at: https://policies.google.com/technologies/cookies");
        };

        // Find the theme and click it to change the theme.
        if !cloned_cfg.theme.is_empty() {
            let theme_name = cloned_cfg.theme.to_lowercase();

            match find_theme_toggle_button(&tab, &theme_name) {
                Some(selector) => {
                    let click_button = format!("document.querySelector('{selector}').click()");
                    tab.evaluate(&click_button, false)?;
                }
                None => {
                    if cfg!(debug_assertions) {
                        panic!("Unable to find theme {theme_name}.")
                    } else {
                        println!("Unable to find theme {theme_name}, return to default one.")
                    }
                }
            }
        }

        // Generate the PDF.
        let generated_pdf = match page.print_to_pdf(Some(pdf_opts)) {
            Ok(output) => output,
            Err(e) => {
                if time == cloned_cfg.trying_times {
                    panic!("{}, so PDF generation failed!", e);
                }
                println!("{}, retrying after {} seconds...", e, time);
                thread::sleep(Duration::from_secs(time));
                continue;
            }
        };

        if let Some(parent) = generated_pdf_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(generated_pdf_path, generated_pdf)?;
        break;
    }

    Ok(())
}

fn select_chapters_for_pdf<'a>(
    book: &'a mdbook_core::book::Book,
    omit_chapters: &[String],
    print_chapters: &[String],
) -> Vec<&'a mdbook_core::book::Chapter> {
    let omit_chapters = configured_chapter_paths(omit_chapters);
    let print_chapters = configured_chapter_paths(print_chapters);

    book.chapters()
        .filter(|chapter| {
            let in_print_chapters = chapter_matches_config(chapter, &print_chapters);
            if in_print_chapters {
                return true;
            }

            if !print_chapters.is_empty() {
                return false;
            }

            !chapter_matches_config(chapter, &omit_chapters)
        })
        .collect()
}

fn configured_chapter_paths(paths: &[String]) -> HashSet<String> {
    paths
        .iter()
        .map(|path| normalize_chapter_path(Path::new(path)))
        .collect()
}

fn chapter_matches_config(
    chapter: &mdbook_core::book::Chapter,
    configured_paths: &HashSet<String>,
) -> bool {
    let source_path_matches = chapter
        .source_path
        .as_ref()
        .map(|path| configured_paths.contains(&normalize_chapter_path(path)))
        .unwrap_or(false);
    let rendered_path_matches = chapter
        .path
        .as_ref()
        .map(|path| configured_paths.contains(&normalize_chapter_path(path)))
        .unwrap_or(false);

    source_path_matches || rendered_path_matches
}

fn normalize_chapter_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn chapter_pdf_output_path(
    pdf_destination: &Path,
    chapter: &mdbook_core::book::Chapter,
    chapter_index: usize,
) -> PathBuf {
    let mut output_dir = pdf_destination.join("chapters");
    if let Some(chapter_dir) = chapter.path.as_ref().and_then(|path| path.parent()) {
        if !chapter_dir.as_os_str().is_empty() {
            output_dir = output_dir.join(chapter_dir);
        }
    }

    output_dir.join(format!(
        "{}-{}.pdf",
        chapter_pdf_number(chapter, chapter_index),
        slugify_chapter_name(&chapter.name)
    ))
}

fn chapter_pdf_number(chapter: &mdbook_core::book::Chapter, chapter_index: usize) -> String {
    if let Some(number) = &chapter.number {
        let parts: Vec<String> = number.iter().map(|part| format!("{part:02}")).collect();
        if !parts.is_empty() {
            return parts.join("-");
        }
    }

    format!("{:02}", chapter_index + 1)
}

fn chapter_html_path(
    pdf_destination: &Path,
    chapter: &mdbook_core::book::Chapter,
) -> Option<PathBuf> {
    let mut chapter_html_path = chapter.path.clone()?;
    if chapter_html_path.file_name() == Some(OsStr::new("README.md")) {
        chapter_html_path.set_file_name("index.html");
    } else {
        chapter_html_path.set_extension("html");
    }

    Some(
        pdf_destination
            .parent()?
            .join("html")
            .join(chapter_html_path),
    )
}

fn slugify_chapter_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in name.chars() {
        if character.is_alphanumeric() {
            for lowercase in character.to_lowercase() {
                slug.push(lowercase);
            }
            last_was_separator = false;
        } else if !last_was_separator && !slug.is_empty() {
            slug.push('-');
            last_was_separator = true;
        }
    }

    if last_was_separator {
        slug.pop();
    }

    if slug.is_empty() {
        "chapter".to_string()
    } else {
        slug
    }
}

/// Try to find the theme toggle button by the theme name with different selector styles.
fn find_theme_toggle_button(tab: &headless_chrome::Tab, theme_name: &str) -> Option<String> {
    // First try the new selector style (since mdBook v0.5.0).
    let selector_new = format!("button.theme#mdbook-theme-{theme_name}");
    if tab.find_element(&selector_new).is_ok() {
        return Some(selector_new);
    }
    // Fallback to old selector style (for mdBook versions before v0.5.0).
    let selector_old = format!("button.theme#{theme_name}");
    if tab.find_element(&selector_old).is_ok() {
        return Some(selector_old);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdbook_core::book::{Book, BookItem, Chapter, SectionNumber};

    #[test]
    fn selected_chapters_omit_excludes_and_print_overrides_omit() {
        let book = Book::new_with_items(vec![
            BookItem::Chapter(Chapter::new(
                "An Example",
                String::new(),
                "chapter_1.md",
                vec![],
            )),
            BookItem::Chapter(Chapter::new(
                "Another Example",
                String::new(),
                "chapter_2.md",
                vec![],
            )),
            BookItem::Chapter(Chapter::new(
                "One more",
                String::new(),
                "chapter_3.md",
                vec![],
            )),
        ]);

        let selected = select_chapters_for_pdf(
            &book,
            &["chapter_2.md".to_string(), "chapter_3.md".to_string()],
            &["chapter_3.md".to_string()],
        );

        let names: Vec<_> = selected
            .iter()
            .map(|chapter| chapter.name.as_str())
            .collect();
        assert_eq!(names, vec!["One more"]);
    }

    #[test]
    fn chapter_pdf_path_uses_summary_name_for_human_readable_filename() {
        let mut chapter = Chapter::new("An Example", String::new(), "chapter_1.md", vec![]);
        chapter.number = Some(SectionNumber::new(vec![1]));

        let output_path = chapter_pdf_output_path(Path::new("book/pdf"), &chapter, 0);

        assert_eq!(
            output_path,
            PathBuf::from("book/pdf/chapters/01-an-example.pdf")
        );
    }

    #[test]
    fn chapter_pdf_path_preserves_summary_number_for_selected_subsets() {
        let mut chapter = Chapter::new("One more", String::new(), "chapter_3.md", vec![]);
        chapter.number = Some(SectionNumber::new(vec![3]));

        let output_path = chapter_pdf_output_path(Path::new("book/pdf"), &chapter, 0);

        assert_eq!(
            output_path,
            PathBuf::from("book/pdf/chapters/03-one-more.pdf")
        );
    }

    #[test]
    fn chapter_pdf_path_preserves_chapter_directory_structure() {
        let mut chapter = Chapter::new("Install", String::new(), "guide/install.md", vec![]);
        chapter.number = Some(SectionNumber::new(vec![2, 1]));

        let output_path = chapter_pdf_output_path(Path::new("book/pdf"), &chapter, 0);

        assert_eq!(
            output_path,
            PathBuf::from("book/pdf/chapters/guide/02-01-install.pdf")
        );
    }

    #[test]
    fn chapter_html_path_uses_the_html_book_next_to_pdf_destination() {
        let chapter = Chapter::new("Install", String::new(), "guide/install.md", vec![]);

        let html_path = chapter_html_path(Path::new("book/pdf"), &chapter).unwrap();

        assert_eq!(html_path, PathBuf::from("book/html/guide/install.html"));
    }

    #[test]
    fn chapter_html_path_maps_readme_chapters_to_index_html() {
        let root_chapter = Chapter::new("Home", String::new(), "README.md", vec![]);
        let nested_chapter = Chapter::new("Guide", String::new(), "guide/README.md", vec![]);

        let root_html_path = chapter_html_path(Path::new("book/pdf"), &root_chapter).unwrap();
        let nested_html_path = chapter_html_path(Path::new("book/pdf"), &nested_chapter).unwrap();

        assert_eq!(root_html_path, PathBuf::from("book/html/index.html"));
        assert_eq!(
            nested_html_path,
            PathBuf::from("book/html/guide/index.html")
        );
    }

    #[test]
    fn static_site_url_rewrite_preserves_nested_links_inside_the_book() {
        let test_dir = std::env::temp_dir().join(format!(
            "mdbook-pdf-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let chapter_path = test_dir.join("book/html/guide/install.html");
        fs::create_dir_all(chapter_path.parent().unwrap()).unwrap();
        fs::write(
            &chapter_path,
            r#"<html><head><script>const path_to_root = "../";</script></head><body><a href="../intro.html">Intro</a><a href="../../outside.html">Outside</a></body></html>"#,
        )
        .unwrap();

        let mut cfg = PrintOptions::default();
        cfg.static_site_url = "https://example.test/book".to_string();

        prepare_html_for_pdf(&chapter_path, &cfg, "").unwrap();

        let contents = fs::read_to_string(&chapter_path).unwrap();
        fs::remove_dir_all(&test_dir).unwrap();

        assert!(contents.contains(r#"href="../intro.html""#));
        assert!(contents.contains(r#"href="https://example.test/book/../../outside.html""#));
    }
}
