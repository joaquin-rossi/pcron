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
- `@ days hours` makes the delay count only while that window is open, in local time: days like `mon-fri` or `sat,sun`, hours like `9-18` or `9:30-12,14-18` (either can be left out).
- `$ ...` runs the rest of the line; a `$$` block ends at a line containing only `$$`.
- `NAME=value` lines set environment variables for the commands below them (no `$` expansion, like cron).
- Scripts run with `$SHELL -c`, where `SHELL` can be set in the tab (`/bin/sh` by default). A run starts even if the previous one is still going, like cron.
- `#` starts a comment.

Distributions: `beta(alpha, beta)`, `cauchy(median, scale)`, `chi_squared(k)`, `exp(lambda)`, `exp1()`, `fisher_f(m, n)`, `frechet(location, scale, shape)`, `gamma(shape, scale)`, `gumbel(location, scale)`, `inverse_gaussian(mean, shape)`, `log_normal(mu, sigma)`, `normal(mean, std_dev)`, `normal_inverse_gaussian(alpha, beta)`, `pareto(scale, shape)`, `pert(min, max, mode, shape)`, `poisson(lambda)`, `skew_normal(location, scale, shape)`, `standard_normal()`, `standard_uniform()`, `student_t(nu)`, `triangular(min, max, mode)`, `uniform(min, max)`, `weibull(scale, shape)`.

## Server

- Every delay is clamped to `--min-delay` (default 1s) and `--max-delay` (default 1 year).
- It logs to stdout, including the scripts' output (`--output discard` to drop it).
- `SIGHUP` reloads the tab: unchanged commands keep their timers, and an invalid tab is ignored.
- `SIGTERM` / `SIGINT` stop it, giving running scripts `--shutdown-timeout` seconds (default 10) to finish.

See `pcron <command> --help` for all the options.
