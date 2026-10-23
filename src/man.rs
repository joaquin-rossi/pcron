use anyhow::Result;
use clap_mangen::Man;
use std::io::Write;

// what clap knows about (commands and options), plus the long help converted to roff
pub fn render(cmd: clap::Command, w: &mut dyn Write) -> Result<()> {
    let man = Man::new(cmd.clone());
    man.render_title(w)?;
    man.render_name_section(w)?;
    man.render_synopsis_section(w)?;
    writeln!(w, ".SH DESCRIPTION")?;
    writeln!(
        w,
        "Runs commands at random intervals: after each run, the delay until the next one is \
         drawn from a probability distribution, given for each command in the tab."
    )?;
    man.render_options_section(w)?;

    writeln!(w, ".SH COMMANDS")?;
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        writeln!(w, ".SS \"pcron {}\"", sub.get_name())?;
        if let Some(about) = sub.get_about() {
            writeln!(w, "{}", escape(&about.to_string()))?;
        }
        let mut options = Vec::new();
        Man::new(sub.clone()).render_options_section(&mut options)?;
        // already under the command's subsection
        let options = String::from_utf8(options)?.replacen(".SH OPTIONS\n", "", 1);
        w.write_all(options.as_bytes())?;
    }

    if let Some(help) = cmd.get_after_long_help() {
        w.write_all(help_to_roff(&help.to_string()).as_bytes())?;
    }
    writeln!(w, ".SH \"SEE ALSO\"\n.BR crontab (5)")?;
    man.render_version_section(w)?;
    man.render_authors_section(w)?;
    Ok(())
}

const CODE_INDENT: usize = 6;

// the conventions of help.txt: `HEADING:` lines are sections, `- ` starts an item, lines
// indented by CODE_INDENT are code, and blank lines separate paragraphs
fn help_to_roff(text: &str) -> String {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let is_code = |line: &str| !line.trim().is_empty() && indent(line) >= CODE_INDENT;
    let lines: Vec<&str> = text.lines().collect();

    let mut out = String::new();
    let mut in_code = false;
    let mut new_paragraph = false;
    for (i, &line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            // a blank line between code lines is part of the code
            let more_code = lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| is_code(l));
            if in_code && more_code {
                out.push('\n');
            } else {
                new_paragraph = true;
            }
            continue;
        }

        if is_code(line) {
            if !in_code {
                out.push_str(".PP\n.RS\n.nf\n");
                in_code = true;
            }
            out.push_str(&escape(&line[CODE_INDENT..]));
            out.push('\n');
            continue;
        }
        if in_code {
            out.push_str(".fi\n.RE\n");
            in_code = false;
        }

        let text = line.trim();
        if indent(line) == 0 && text.ends_with(':') {
            out.push_str(&format!(".SH \"{}\"\n", text.trim_end_matches(':')));
        } else if let Some(item) = text.strip_prefix("- ") {
            out.push_str(".IP \\(bu 2\n");
            out.push_str(&escape(item));
            out.push('\n');
        } else {
            if new_paragraph {
                out.push_str(".PP\n");
            }
            out.push_str(&escape(text));
            out.push('\n');
        }
        new_paragraph = false;
    }
    if in_code {
        out.push_str(".fi\n.RE\n");
    }
    out
}

fn escape(text: &str) -> String {
    let text = text.replace('\\', "\\e").replace('-', "\\-");
    // a leading . or ' would be read as a request
    if text.starts_with(['.', '\'']) {
        format!("\\&{text}")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_conventions() {
        let help = "\
HEADING:
  Some text
  that goes on.

      code -x
      .hidden

      more code

  - an item
    continued
  - another

  After the list.
";
        assert_eq!(
            help_to_roff(help),
            "\
.SH \"HEADING\"
Some text
that goes on.
.PP
.RS
.nf
code \\-x
\\&.hidden

more code
.fi
.RE
.IP \\(bu 2
an item
continued
.IP \\(bu 2
another
.PP
After the list.
"
        );
    }
}
