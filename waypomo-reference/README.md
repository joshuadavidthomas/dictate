# waypomo

waypomo is a shameless clone/interpretation/re-implementation of
[Pomodouroboros][], from the inimitable [glyph][].

in a nutshell: it's a pomodoro timer, but instead of manually triggering the
start of a pomodoro, the timer runs constantly. this is intended to help fight
adhd time blindness. personally, i find it difficult to run that first pomodoro
of the day and really get the ball rolling. i set waypomo to run as soon as i
log in to help mitigate this.

you have the option to set a free-text "intention" per pomodoro – basically,
what you would like to focus on or get done. at the end of the pomorodo,
waypomo will ask if you stayed focused – or, if you didn't have an intention
set, it will suggest that perhaps you should.

the original Pomodouroboros has some very nice features, such as:
- keeping track of how many pomodoros you've done in a day, and how many had
  intentions set, how many you focused during, etc.
- displaying at-a-glance information about how many pomodoros you've focused on
  so far in a given day
- a very nice list view showing the data it's collected

waypomo currently does none of this. perhaps in the future it will.

## system requirements

waypomo, as the name suggests, targets wayland compositors, and specifically
those that implement [wlr-layer-shell-unstable-v1][]. many compositors do, with
the notable exclusion of mutter (gnome).

waypomo is written in rust, and uses relm4 and gtk4 for its ui.

waypomo is developed on linux. it should run on other OSes that can run
wayland. freebsd in particular should work but is not tested yet.

## running it

```shell
cargo run --release
```

## configuration

there is an example configuration file at `./config/config.kdl`. waypomo will
look for this file in your xdg config home, which will usually resolve to
`~/.config/waypomo/config.kdl`.

there is a stylesheet at `./config/style.css` which dictates the look and feel
of everything in the app. currently, this stylesheet is embedded in the
executable at compile-time. in the future, i would like to have waypomo load an
additional user stylesheet from the same XDG configuration directory.

## future plans

loose thoughts rather than anything concrete:

- we should probably be using a proper logging module and clean up the
  `println!()` scattered about.
- currently, waypomo outputs a "report" of every work block to stdout. in the
  future, it would be nice to log/write this to a file for tracking. perhaps
  sqlite? perhaps a JSON file or something simple?
- the Pomodouroboros feature that displays how many work blocks have been
  accomplished on a given day and how many of them had an intention set, how many
  were "achieved" vs "focused" vs "distracted" - this could be a nice little
  additional widget on the timer window.
- it would be nice if the timer widget displayed on all monitors instead of
  just one. it's unclear to me how we'll do this with relm4's concept of a
  "model", since the model for the timer window right now drives the entire app.
  ideally we'd split it into "headless" app logic, and then the timer windows
  would be simple views that display some of the data therein. it's unclear to me
  how to best implement this with relm4, however.
- user styles!
- live reloading of configuration and also styles!
- notifications or modal dialogs for the start of time blocks or at the very
  least after a long break
- `on-start` commands for work/short-break/long-break blocks.
- `on-completion` commands for short and long breaks (currently only
  implemented for work blocks)

## development

pull requests and issues both welcome. waypomo is largely a tool i'm writing to
scratch my own itch but i'll be very happy if other folks also find it
helpful/useful. if there are small things that would improve the workflow for
you, please let me know - or, better yet, hop right in and poke around in the
code yourself.

this is my first project using relm4 and gtk more generally, so there's a good
chance i'm doing things less-than-idiomatically or just straight up
inefficiently. i welcome corrections and guidance on this!

on rustfmt: i have several times tried to make a rustfmt configuration that
formats code in a way that i do not detest. it hasn't happened yet. feel free
to give it a shot.

[glyph]: https://github.com/glyph
[Pomodouroboros]: https://github.com/glyph/Pomodouroboros/
[wlr-layer-shell-unstable-v1]: https://wayland.app/protocols/wlr-layer-shell-unstable-v1
