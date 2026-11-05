# Contributing to Podroid

Please also read the [code of conduct](CODE_OF_CONDUCT.md).

Podroid is an unofficial fork of [touchHLE](https://github.com/touchHLE/touchHLE) focused on Android. **Please report problems with Podroid here, not to touchHLE, and never submit code from this fork to touchHLE.** It doesn't accept AI-generated code, and this fork contains some.

## Issues

Please bear in mind that many apps don't work in Podroid yet. Issues about broken apps are most useful if one of these applies:

- This specific version of the app worked in a previous version of Podroid, and you can reproduce the problem.
- The app is partially working (e.g. it reaches the menu but the main game doesn't work). The app's splash screen (`Default.png`) showing up doesn't mean it's partially working.

Please include your Podroid version, your device model and the app's version, and attach `Podroid_log.txt` if you can. Don't attach app files.

## Pull requests

Get the source code like this:

```
$ git clone --recurse-submodules https://github.com/shawnw67/Podroid.git
$ cd Podroid
```

Fork the repository on GitHub, make a branch based on `trunk`, then open a pull request against `trunk`.

If your pull request has non-trivial changes, please open an issue first so we can figure out if it's likely to be accepted. It would be a shame if your effort had to be wasted.

Try to avoid bundling unrelated changes in one commit. We prefer smaller commits that can be reviewed independently.

## Code contributions

The developer documentation can be found in [the `dev-docs` directory](dev-docs/) and throughout the codebase. At a minimum, you'll probably want to read [the building guide](dev-docs/building.md) and [the coding style guide](dev-docs/code-style.md).

Before opening a pull request:

- Test your changes on a real Android phone. The Android emulator's OpenGL ES 1.1 translation has bugs that real phones don't. Say which device and app you tested with.
- Run [`dev-scripts/format.sh`](dev-scripts/format.sh) and [`dev-scripts/lint.sh`](dev-scripts/lint.sh).
- Run `cargo test`. [Building the integration tests requires downloading LLVM and a custom toolchain](tests/README.md), so you can skip them with `cargo test -- --skip test_app`.

Game-specific workarounds should check that they are running the expected code (e.g. by comparing a function's start bytes) before patching or calling into the game.

Don't add app files to the repository or bundle them into builds.

### AI assistance

AI assistants may be used to contribute to Podroid, with these conditions:

- Say so in the pull request, and mark the commits involved (e.g. with a `Co-Authored-By` line).
- You're responsible for what you submit: read it, understand it and test it on a real device.
- The copyright rules below apply to AI-generated code too.

### Copyright and reverse engineering

(Please also read the copyright rules in the code of conduct.)

⚠️ Be **very** careful about copyright. To put it simply: **don't contribute if you've seen code you shouldn't have seen, don't copy code that isn't yours to copy, and especially don't _secretly_ copy and pretend you didn't**. Any infringement of Apple or other copyrights could threaten the project. **If in doubt, don't do it**, but in particular:

* ⚠️ When implementing an API, rely firstly and primarily on public documentation.
* ⚠️ Do not under any circumstances look at or rely on _leaked_ code, documentation, tools, etc. Material being available somewhere does not mean it is open-source.
* ⚠️ Do not disassemble or decompile components of iOS or other Apple platforms. If you can't figure out how else you would find out how an API should behave, please just don't try to implement it.
* ⚠️ Looking at header files is occasionally necessary, but it should not be your first resort, and you must only use them as a source of simple facts (e.g. what value does a constant have, what type does a type alias resolve to). Do not copy their layout and organization. Do not copy anything you do not need to. Except where the name is part of the ABI or public API, do not copy names.
* ⚠️ Bear in mind that open-source code is still covered by copyright, and so the same caution applies to consulting open-source code. Especially try to avoid looking at the implementation files, unless there is no other option, and do not copy algorithms. (Note however that if it's under a compatible license, we may be able to bring the open-source code into the project _under that license, as a dependency_.)
* ⚠️ If you work or have worked at Apple, or NeXT, or various other organisations, then you may have seen the proprietary source code used in components of iOS. If that's the case, please do not contribute to this project.
* ⚠️ If your employment contract or applicable law in your country means that you don't own the copyright on code you want to contribute to this project, or if for some other reason you may need permission from your employer to contribute to this project: please do obtain that permission before contributing.
