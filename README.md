# Madrillo

The mother armadillo: a native process monitor inspired by conmon.

This repository is under construction. The [implementation plan](PLAN.md)
defines the full scope, success criteria, and required three consecutive clean
conformance passes. No feature completeness or native-platform conformance is
claimed yet.

Madrillo is one Rust package: a minimal shared-types library and a complete
monitor executable. All owned Rust targets forbid unsafe code. Pichi and dillo
are the first integration, while the monitor remains independent of VM APIs.
