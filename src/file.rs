pub mod lexer;
pub mod parser;

use crate::{
    Delay, DelayBounds, DistFloat, Settings, Tab, TabCmd,
    window::{Window, Zone},
};
use parser::Expr;

use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

/// the variables of the tab that configure pcron instead of the scripts
pub const SETTINGS: [&str; 6] = [
    "PCRON_MIN_DELAY",
    "PCRON_MAX_DELAY",
    "PCRON_TIMEOUT",
    "PCRON_SHUTDOWN_TIMEOUT",
    "PCRON_TZ",
    "PCRON_ON_FAILURE",
];

/// a tab is the file `path` plus every `*.tab` in the directory `path.d`, in alphabetical
/// order (either may be missing, not both); each file is parsed on its own, so variables
/// don't carry over from one to the next
pub fn read(path: impl AsRef<Path>) -> Result<Tab> {
    let path = path.as_ref();
    let files = tab_files(path)?;
    ensure!(
        !files.is_empty(),
        "cannot find the tab: neither {} nor {}/*.tab exist",
        path.display(),
        dropin_dir(path).display()
    );
    let texts = files
        .into_iter()
        .map(|file| {
            let text = fs::read_to_string(&file)
                .with_context(|| format!("cannot read tab {}", file.display()))?;
            Ok((file, text))
        })
        .collect::<Result<Vec<_>>>()?;
    parse_files(path, &texts)
}

/// `path.d`, where the rest of the files of the tab `path` are
pub fn dropin_dir(path: &Path) -> PathBuf {
    let mut dir = OsString::from(path.as_os_str());
    dir.push(".d");
    PathBuf::from(dir)
}

/// the files of the tab `path` that exist: `path` itself first, and then `path.d/*.tab`
/// (which leaves out editor backups, and the hidden copies of `pcron edit`)
pub fn tab_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => files.push(path.to_path_buf()),
        Ok(_) => bail!("the tab {} is not a file", path.display()),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot read tab {}", path.display())),
    }

    let dir = dropin_dir(path);
    match fs::read_dir(&dir) {
        Ok(entries) => {
            let mut more: Vec<_> = entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| {
                    let name = p.file_name().unwrap_or_default().to_string_lossy();
                    !name.starts_with('.') && name.ends_with(".tab") && p.is_file()
                })
                .collect();
            more.sort();
            files.extend(more);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    }
    Ok(files)
}

/// the files of the tab `path` (see read), with their contents
pub fn parse_files(path: &Path, files: &[(PathBuf, String)]) -> Result<Tab> {
    // shown relative to the tab's directory: `tab`, `tab.d/backup.tab`
    let base = path.parent().unwrap_or(Path::new(""));
    let mut cmds = Vec::new();
    for (file, text) in files {
        let tab = parse(text).with_context(|| format!("invalid tab {}", file.display()))?;
        let shown = file
            .strip_prefix(base)
            .unwrap_or(file)
            .display()
            .to_string();
        cmds.extend(tab.cmds.into_iter().map(|cmd| TabCmd {
            file: shown.clone(),
            ..cmd
        }));
    }
    check_names(&cmds)?;
    Ok(Tab { cmds })
}

/// names are unique (in the whole tab)
fn check_names(cmds: &[TabCmd]) -> Result<()> {
    let mut names = HashMap::new();
    for cmd in cmds {
        if let Some(name) = &cmd.name {
            if let Some(first) = names.insert(name, cmd.at()) {
                bail!("{}: the name `{name}` is already used at {first}", cmd.at());
            }
        }
    }
    Ok(())
}

/// a lone file
pub fn parse(text: &str) -> Result<Tab> {
    let tokens = lexer::tokenize(text).map_err(|e| anyhow!(e))?;
    let ast = parser::parse(&tokens).map_err(|e| anyhow!(e))?;

    let cmds = ast
        .cmds
        .into_iter()
        .map(|cmd| {
            let delay = build_delay(&cmd.expr)
                .with_context(|| format!("line {}: in `{}`", cmd.line, cmd.expr))?;
            let window = cmd
                .window
                .map(|text| {
                    Window::parse(&text)
                        .map_err(|e| anyhow!("line {}: in `@ {text}`: {e}", cmd.line))
                })
                .transpose()?;
            let settings = settings(&cmd.env).with_context(|| format!("line {}", cmd.line))?;
            let source = text[cmd.span]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            Ok(TabCmd {
                file: String::new(),
                line: cmd.line,
                name: cmd.name,
                source,
                delay,
                window,
                env: cmd.env,
                settings,
                script: cmd.script,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    check_names(&cmds)?;
    Ok(Tab { cmds })
}

// the PCRON_ variables in effect for a command; an empty one means the default
fn settings(env: &[(String, String)]) -> Result<Settings> {
    let mut settings = Settings::default();
    let (mut min, mut max) = (settings.bounds.min, settings.bounds.max);
    for (name, value) in env.iter().filter(|(name, _)| name.starts_with("PCRON_")) {
        let in_setting = || format!("in `{name}={value}`");
        let secs = || seconds(value).with_context(in_setting);
        let duration = |secs: f64| {
            Duration::try_from_secs_f64(secs).map_err(|_| anyhow!("{}: too long", in_setting()))
        };
        let text = value.trim();
        match name.as_str() {
            "PCRON_MIN_DELAY" => min = secs()?.unwrap_or(min),
            "PCRON_MAX_DELAY" => max = secs()?.unwrap_or(max),
            // 0 means no limit too
            "PCRON_TIMEOUT" => {
                settings.timeout = secs()?.filter(|&s| s > 0.0).map(duration).transpose()?;
            }
            "PCRON_SHUTDOWN_TIMEOUT" => {
                if let Some(secs) = secs()? {
                    settings.shutdown_timeout = duration(secs)?;
                }
            }
            "PCRON_TZ" if text.is_empty() => settings.tz = Zone::Local,
            "PCRON_TZ" => {
                settings.tz = Zone::parse(text).map_err(|e| anyhow!("{}: {e}", in_setting()))?;
            }
            "PCRON_ON_FAILURE" => {
                settings.on_failure = Some(text.to_string()).filter(|t| !t.is_empty());
            }
            _ => bail!(
                "unknown setting `{name}` (the PCRON_ ones are {})",
                SETTINGS.join(", ")
            ),
        }
    }
    settings.bounds = DelayBounds::new(min, max).map_err(|e| anyhow!(e))?;
    Ok(settings)
}

// a constant expression, like the arguments of a distribution; None if empty
fn seconds(value: &str) -> Result<Option<f64>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    // the value has a single line, so leave out the "line 1" of the messages
    let unline = |e: String| anyhow!(e.strip_prefix("line 1: ").unwrap_or(&e).to_string());
    let tokens = lexer::tokenize(value).map_err(unline)?;
    let expr = parser::parse_expr(&tokens).map_err(unline)?;
    eval_arg(&expr).map(Some)
}

// constant parts are evaluated now; a constant delay must not be negative (a random one
// is clamped when sampled)
fn build_delay(e: &Expr) -> Result<Delay> {
    let delay = build(e)?;
    if let Delay::Const(v) = delay {
        ensure!(v >= 0.0, "the delay is negative ({v})");
    }
    Ok(delay)
}

fn build(e: &Expr) -> Result<Delay> {
    if !e.is_random() {
        return Ok(Delay::Const(eval(e)?));
    }
    Ok(match e {
        Expr::Num(_) => unreachable!("numbers are not random"),
        Expr::Call(name, args) => {
            let args = args
                .iter()
                .map(eval_arg)
                .collect::<Result<Vec<_>>>()
                .with_context(|| format!("in `{e}`"))?;
            let dist = build_dyn_dist(name, &args).with_context(|| format!("in `{e}`"))?;
            Delay::Dist {
                name: name.clone(),
                args,
                dist,
            }
        }
        Expr::Neg(x) => Delay::Neg(Box::new(build(x)?)),
        Expr::Bin(op, l, r) => Delay::Bin(*op, Box::new(build(l)?), Box::new(build(r)?)),
    })
}

// arguments must be constant and end up non-negative (eval already rejects non-finite
// values)
fn eval_arg(e: &Expr) -> Result<f64> {
    let v = eval(e)?;
    ensure!(v >= 0.0, "`{e}` is negative ({v})");
    Ok(v)
}

// every intermediate result must be finite (catches 1/0, 0^-1, (-8)^(1/3), ...)
fn eval(e: &Expr) -> Result<f64> {
    let v = match e {
        Expr::Num(n) => *n,
        Expr::Call(..) => bail!("`{e}` is random, but distribution arguments must be constant"),
        Expr::Neg(x) => -eval(x)?,
        Expr::Bin(op, l, r) => op.apply(eval(l)?, eval(r)?),
    };
    ensure!(v.is_finite(), "`{e}` is not a finite number ({v})");
    Ok(v)
}

// - beta(alpha, beta)
// - cauchy(median, scale)
// - chi_squared(k)
// - exp(lambda)
// - exp1()
// - fisher_f(m, n)
// - frechet(location, scale, shape)
// - gamma(shape, scale)
// - gumbel(location, scale)
// - inverse_gaussian(mean, shape)
// - log_normal(mu, sigma)
// - normal(mean, std_dev)
// - normal_inverse_gaussian(alpha, beta)
// - pareto(scale, shape)
// - pert(min, max, mode, shape)
// - poisson(lambda)
// - skew_normal(location, scale, shape)
// - standard_normal()
// - standard_uniform()
// - student_t(nu)
// - triangular(min, max, mode)
// - uniform(min, max)
// - weibull(scale, shape)
pub fn build_dyn_dist(name: &str, a: &[f64]) -> Result<Box<dyn DistFloat>> {
    let expect = |n: usize| {
        if a.len() == n {
            Ok(())
        } else {
            Err(anyhow!("`{name}` expects {n} argument(s), got {}", a.len()))
        }
    };

    use rand_distr::{
        Beta, Cauchy, ChiSquared, Exp, Exp1, FisherF, Frechet, Gamma, Gumbel, InverseGaussian,
        LogNormal, Normal, NormalInverseGaussian, Pareto, Pert, Poisson, SkewNormal,
        StandardNormal, StandardUniform, StudentT, Triangular, Uniform, Weibull,
    };
    let d: Box<dyn DistFloat> = match name {
        "beta" => {
            expect(2)?;
            Box::new(Beta::new(a[0], a[1])?)
        }
        "const" => bail!("there is no `const`: for a fixed delay write just the number"),
        "cauchy" => {
            expect(2)?;
            Box::new(Cauchy::new(a[0], a[1])?)
        }
        "chi_squared" | "chisquared" | "chi2" => {
            expect(1)?;
            Box::new(ChiSquared::new(a[0])?)
        }
        "exp" | "exponential" => {
            expect(1)?;
            Box::new(Exp::new(a[0])?)
        }
        "exp1" => {
            expect(0)?;
            Box::new(Exp1)
        }
        "fisher_f" | "fisherf" | "f" => {
            expect(2)?;
            Box::new(FisherF::new(a[0], a[1])?)
        }
        "frechet" => {
            expect(3)?;
            Box::new(Frechet::new(a[0], a[1], a[2])?)
        }
        "gamma" => {
            expect(2)?;
            Box::new(Gamma::new(a[0], a[1])?)
        }
        "gumbel" => {
            expect(2)?;
            Box::new(Gumbel::new(a[0], a[1])?)
        }
        "inverse_gaussian" | "inversegaussian" | "wald" => {
            expect(2)?;
            Box::new(InverseGaussian::new(a[0], a[1])?)
        }
        "log_normal" | "lognormal" => {
            expect(2)?;
            Box::new(LogNormal::new(a[0], a[1])?)
        }
        "normal" => {
            expect(2)?;
            Box::new(Normal::new(a[0], a[1])?)
        }
        "normal_inverse_gaussian" | "normalinversegaussian" | "nig" => {
            expect(2)?;
            Box::new(NormalInverseGaussian::new(a[0], a[1])?)
        }
        "pareto" => {
            expect(2)?;
            Box::new(Pareto::new(a[0], a[1])?)
        }
        "pert" => {
            expect(4)?;
            Box::new(Pert::new(a[0], a[1]).with_shape(a[3]).with_mode(a[2])?)
        }
        "poisson" => {
            expect(1)?;
            Box::new(Poisson::new(a[0])?)
        }
        "skew_normal" | "skewnormal" => {
            expect(3)?;
            Box::new(SkewNormal::new(a[0], a[1], a[2])?)
        }
        "standard_normal" | "std_normal" => {
            expect(0)?;
            Box::new(StandardNormal)
        }
        "standard_uniform" | "std_uniform" => {
            expect(0)?;
            Box::new(StandardUniform)
        }
        "student_t" | "studentt" | "t" => {
            expect(1)?;
            Box::new(StudentT::new(a[0])?)
        }
        "triangular" => {
            expect(3)?;
            Box::new(Triangular::new(a[0], a[1], a[2])?)
        }
        "uniform" => match a.len() {
            2 => Box::new(Uniform::new_inclusive(a[0], a[1])?),
            1 => bail!(
                "`uniform` expects 2 arguments (min, max); for a fixed delay write just the number"
            ),
            n => bail!("`uniform` expects 2 arguments, got {n}"),
        },
        "weibull" => {
            expect(2)?;
            Box::new(Weibull::new(a[0], a[1])?)
        }
        _ => bail!("unknown distribution `{name}`"),
    };

    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pert_args_are_min_max_mode_shape() {
        // mode=1 is inside [0, 10]; with mode and shape swapped it would be out of range
        build_dyn_dist("pert", &[0.0, 10.0, 1.0, 20.0]).unwrap();
    }

    fn eval_str(expr: &str) -> Result<f64> {
        let tokens = lexer::tokenize(&format!("{expr} $ true")).unwrap();
        let ast = parser::parse(&tokens).unwrap();
        eval(&ast.cmds[0].expr)
    }

    fn delay(tab: &str) -> Delay {
        parse(tab).unwrap().cmds.remove(0).delay
    }

    #[test]
    fn delay_expressions() {
        // constant parts are folded
        assert_eq!(delay("60 * 60 $ a").to_string(), "3600");
        let d = delay("2 * 1800 + exp(1/600) $ a");
        assert_eq!(d.to_string(), "3600 + exp(0.0016666666666666668)");
        assert!((0..100).all(|_| d.sample() >= 3600.0));

        let d = delay("exp(1) + exp(2) - (uniform(0, 1) * 2) $ a");
        assert_eq!(d.to_string(), "(exp(1) + exp(2)) - (uniform(0, 1) * 2)");
        let mean = (0..20_000).map(|_| d.sample()).sum::<f64>() / 20_000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");

        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("exp(uniform(1, 2)) $ a").contains("arguments must be constant"));
        assert!(err("1 - 2 $ a").contains("the delay is negative"));
        // random parts may go below 0; they are clamped when sampled
        parse("-exp(1) $ a").unwrap();
    }

    #[test]
    fn arithmetic() {
        let cases = [
            ("1/60", 1.0 / 60.0),
            ("1 + 2 * 3", 7.0),
            ("(1 + 2) * 3", 9.0),
            ("10 - 4 - 3", 3.0),
            ("8 / 4 / 2", 1.0),
            ("2^3^2", 512.0),
            ("-2^2", -4.0),
            ("2^-1", 0.5),
            ("+3 - -2", 5.0),
            ("1.5e2", 150.0),
        ];
        for (arg, want) in cases {
            assert_eq!(eval_str(arg).unwrap(), want, "{arg}");
        }
    }

    #[test]
    fn non_finite_is_an_error() {
        for arg in ["1/0", "0^-1", "(-8)^(1/3)", "0/0", "1e400"] {
            let err = format!("{:#}", eval_str(arg).unwrap_err());
            assert!(err.contains("not a finite number"), "{arg}: {err}");
        }
    }

    #[test]
    fn arg_must_be_non_negative() {
        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("exp(1 - 2) $ true").contains("`1 - 2` is negative"));
        parse("exp(1/60) $ true\nnormal(-1 + 2, 0) $ true").unwrap();
    }

    #[test]
    fn constant_and_uniform() {
        let d = delay("42.5 $ a");
        assert!((0..10).all(|_| d.sample() == 42.5));
        let err = format!("{:#}", parse("const(1) $ a").err().unwrap());
        assert!(err.contains("write just the number"), "{err}");

        let d = build_dyn_dist("uniform", &[1.0, 2.0]).unwrap();
        assert!(
            (0..100)
                .map(|_| d.sample())
                .all(|x| (1.0..=2.0).contains(&x))
        );

        let err = build_dyn_dist("uniform", &[10.0])
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("write just the number"), "{err}");
    }

    #[test]
    fn names_and_windows() {
        let tab = parse("a: 60 @ mon-fri 9-18 $ x\nb: 60 $ y\n60 $ z").unwrap();
        let names: Vec<_> = tab.cmds.iter().map(|c| c.name.as_deref()).collect();
        assert_eq!(names, [Some("a"), Some("b"), None]);
        assert_eq!(
            tab.cmds[0].window.as_ref().unwrap().to_string(),
            "mon-fri 9-18"
        );

        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(
            err("a: 60 $ x\n\na: 60 $ y")
                .contains("line 3: the name `a` is already used at line 1")
        );
        assert!(err("60 @ funday $ x").contains("line 1: in `@ funday`: unknown day"));
    }

    #[test]
    fn settings_apply_to_the_commands_below() {
        let tab = parse(
            "\
1 $ x
PCRON_MIN_DELAY=0.5
PCRON_TIMEOUT=2*60
1 $ y
PCRON_TIMEOUT=
PCRON_MAX_DELAY=3600
PCRON_SHUTDOWN_TIMEOUT=3
1 $ z",
        )
        .unwrap();
        let s: Vec<_> = tab.cmds.iter().map(|c| c.settings.clone()).collect();
        assert_eq!(s[0], Settings::default());
        assert_eq!(s[1].bounds.min, 0.5);
        assert_eq!(s[1].timeout, Some(Duration::from_secs(120)));
        assert_eq!(s[2].bounds, DelayBounds::new(0.5, 3600.0).unwrap());
        assert_eq!(s[2].timeout, None);
        assert_eq!(s[2].shutdown_timeout, Duration::from_secs(3));

        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("PCRON_TIMOUT=1\n1 $ x").contains("unknown setting `PCRON_TIMOUT`"));
        assert!(err("PCRON_TIMEOUT=exp(1)\n1 $ x").contains("must be constant"));
        assert!(err("PCRON_TIMEOUT=1 2\n1 $ x").contains("expected the end of the value"));
        assert!(err("PCRON_MIN_DELAY=-1\n1 $ x").contains("is negative"));
        assert!(
            err("PCRON_MIN_DELAY=9\nPCRON_MAX_DELAY=5\n\n1 $ x")
                .contains("line 4: PCRON_MAX_DELAY must be")
        );
    }

    #[test]
    fn text_settings() {
        let tab = parse(
            "\
PCRON_TZ=Europe/Madrid
PCRON_ON_FAILURE=notify-send \"$PCRON_NAME failed\"
1 $ x
PCRON_TZ=
PCRON_ON_FAILURE=
1 $ y",
        )
        .unwrap();
        let (a, b) = (&tab.cmds[0].settings, &tab.cmds[1].settings);
        assert_eq!(a.tz.to_string(), "Europe/Madrid");
        assert_eq!(
            a.on_failure.as_deref(),
            Some("notify-send \"$PCRON_NAME failed\"")
        );
        assert_eq!((b.tz, b.on_failure.as_deref()), (Zone::Local, None));

        let err = format!("{:#}", parse("PCRON_TZ=Nowhere\n1 $ x").err().unwrap());
        assert!(
            err.contains("in `PCRON_TZ=Nowhere`: unknown time zone"),
            "{err}"
        );
    }

    #[test]
    fn a_tab_is_a_file_and_its_directory() {
        let dir = std::env::temp_dir().join(format!("pcron-tab-test-{}", std::process::id()));
        let tab = dir.join("tab");
        let d = dropin_dir(&tab);
        fs::create_dir_all(&d).unwrap();
        let write = |p: &Path, text: &str| fs::write(p, text).unwrap();
        let files = |tab: &Tab| -> Vec<String> { tab.cmds.iter().map(TabCmd::at).collect() };

        assert!(
            read(&tab)
                .err()
                .unwrap()
                .to_string()
                .contains("cannot find the tab")
        );

        // just the directory, in order, without backups or hidden files
        write(&d.join("b.tab"), "X=1\n1 $ b");
        write(&d.join("a.tab"), "\n1 $ a");
        write(&d.join("a.tab~"), "nope");
        write(&d.join(".a.tab.1234.tmp"), "nope");
        write(&d.join("notes.txt"), "nope");
        assert_eq!(
            files(&read(&tab).unwrap()),
            ["tab.d/a.tab:2", "tab.d/b.tab:2"]
        );

        // the file comes first, and its variables don't reach the directory
        write(&tab, "X=2\n1 $ main");
        let t = read(&tab).unwrap();
        assert_eq!(files(&t), ["tab:2", "tab.d/a.tab:2", "tab.d/b.tab:2"]);
        let env: Vec<_> = t.cmds.iter().map(|c| c.env.len()).collect();
        assert_eq!(env, [1, 0, 1]);

        // names are unique across files
        write(&d.join("c.tab"), "main: 1 $ x");
        write(&tab, "main: 1 $ main");
        let err = format!("{:#}", read(&tab).err().unwrap());
        assert!(
            err.contains("tab.d/c.tab:1: the name `main` is already used at tab:1"),
            "{err}"
        );

        // an error says which file
        write(&d.join("c.tab"), "oops");
        let err = format!("{:#}", read(&tab).err().unwrap());
        assert!(err.contains("c.tab: line 1:"), "{err}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_name_the_expression() {
        let err = |tab: &str| format!("{:#}", parse(tab).err().unwrap());
        assert!(err("foo(1) $ true").contains("unknown distribution `foo`"));
        assert!(err("normal(1) $ true").contains("in `normal(1)`"));
        assert!(err("beta(-1, 2) $ true").contains("in `beta(-1, 2)`"));
    }
}
