# pcron: probabilistic cron

Runs commands at random intervals: after each run, the delay until the next one is drawn from a probability distribution.

Original idea: [https://news.ycombinator.com/item?id=46401425](https://news.ycombinator.com/item?id=46401425)

## Usage

```sh
pcron edit      # edit the tab
pcron list      # list the commands
pcron server    # run them
```

The tab lives in `$XDG_CONFIG_HOME/pcron/tab` (or `~/.config/pcron/tab`); use `--tab` or `PCRON_TAB` for another one.

## Tab

Each command is the delay before its next run, in seconds, followed by a script:

```sh
# about once a minute, on average (a Poisson process)
exp(1/60) $ echo hi >> /tmp/pcron.log

# every 30 to 40 minutes of working hours
stretch: uniform(30, 40) * 60 @ mon-fri 9-18 $ notify-send stretch

# every 23 to 25 hours
notes: 24*3600 + normal(0, 1800) $$
    cd ~/notes
    git commit -am "$(date)" && git push
$$
```

- The delay is made of numbers and distributions, with `+ - * / ^` and parentheses. A distribution is sampled at every run; its arguments must be constant and non-negative.
- `name:` before the delay labels the command in the logs and in `pcron list` (optional, unique).
- `@ days hours` makes the delay count only while that window is open, in local time (or `PCRON_TZ`): days like `mon-fri` or `sat,sun`, hours like `9-18` or `9:30-12,14-18` (either can be left out).
- `$ ...` runs the rest of the line; a `$$` block ends at a line containing only `$$`.
- `NAME=value` lines set environment variables for the commands below them (no `$` expansion, like cron).
- `PCRON_*` variables configure pcron for the commands below them (scripts don't get them):
  - `PCRON_MIN_DELAY` and `PCRON_MAX_DELAY` clamp every sampled delay (default 1s and 1 year);
  - `PCRON_TIMEOUT` sends SIGTERM to a script that runs longer than that (default: no limit);
  - `PCRON_SHUTDOWN_TIMEOUT` sends SIGKILL that long after a SIGTERM (default 60);
  - `PCRON_TZ` is the time zone of the windows, like `Europe/Madrid` (default: local);
  - `PCRON_ON_FAILURE` runs when a script fails (exits with an error, is killed, times out or cannot start), with `PCRON_NAME`, `PCRON_REASON`, `PCRON_EXIT`, `PCRON_OUTPUT` and more set; for example `PCRON_ON_FAILURE=notify-send "pcron: $PCRON_NAME failed"`.
- Scripts run with `$SHELL -c`, where `SHELL` can be set in the tab (`/bin/sh` by default). A run starts even if the previous one is still going, like cron.
- `#` starts a comment.

Distributions: `beta(alpha, beta)`, `cauchy(median, scale)`, `chi_squared(k)`, `exp(lambda)`, `exp1()`, `fisher_f(m, n)`, `frechet(location, scale, shape)`, `gamma(shape, scale)`, `gumbel(location, scale)`, `inverse_gaussian(mean, shape)`, `log_normal(mu, sigma)`, `normal(mean, std_dev)`, `normal_inverse_gaussian(alpha, beta)`, `pareto(scale, shape)`, `pert(min, max, mode, shape)`, `poisson(lambda)`, `skew_normal(location, scale, shape)`, `standard_normal()`, `standard_uniform()`, `student_t(nu)`, `triangular(min, max, mode)`, `uniform(min, max)`, `weibull(scale, shape)`.

`pcron list --simulate 7d` shows when the commands would run in the next week.

## Server

- It logs to stdout, including the scripts' output (`--output discard` to drop it).
- It saves the next run of each command in `$XDG_STATE_HOME/pcron/`, so a restart resumes them; a run that was missed while it was down happens right away.
- `SIGHUP` reloads the tab: unchanged commands keep their timers, and an invalid tab is ignored.
- `SIGTERM` / `SIGINT` stop it: running scripts get SIGTERM, and are killed after `PCRON_SHUTDOWN_TIMEOUT` or when their `PCRON_TIMEOUT` runs out, whichever comes first.

See `pcron <command> --help` for all the options.
