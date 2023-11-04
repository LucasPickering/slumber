# Changelog

## [Unreleased]

### Added

- Add top-level collection `id` field
  - Needed in order to give each collection its own history file
- Disable mouse capture to allow text highlighting [#17](https://github.com/LucasPickering/slumber/issues/17)
- Add keybinding (F2) to send request from any view

### Fixed

- Differentiate history between different collections [#10](https://github.com/LucasPickering/slumber/issues/10)
- Ensure ctrl-c can't get eaten by text boxes (it guarantees exit now) [#18](https://github.com/LucasPickering/slumber/issues/18)

### Changed

- Adjust size of profile list dynamically based on number of profiles
- Use structured table display format for query parameters and headers
- Tweak list and tab styling

## [0.4.0] - 2023-11-02

### Added

- Request and response panes can now be fullscreened and scrolled [#14](https://github.com/LucasPickering/slumber/issues/14)

### Removed

- Removed `Chain.name` field in config

### Changed

- All modals now use a shared queue

### Fixed

- Initially selected recipe loads most recent response [#13](https://github.com/LucasPickering/slumber/issues/13)

## [0.3.1] - 2023-10-22

Initial distributed release!
