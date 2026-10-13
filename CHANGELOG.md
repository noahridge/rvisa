# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `rvisa-core`: `ResourceName` parser and formatter, the `Session`,
  `SessionExt` and `Backend` traits, `Error`, attributes, events, VISA
  search patterns and an IEEE 488.2 block reader.
- `rvisa-mock`: scripted mock backend, `RecordingSession` and
  `ReplaySession` with a JSON Lines trace format.
- `rvisa-conformance`: backend-agnostic conformance suite.
- `rvisa`: `ResourceManager` with a backend registry and priority selection;
  mock backend behind the `mock` feature.
