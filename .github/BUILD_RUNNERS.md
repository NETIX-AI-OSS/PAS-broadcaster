# Build worker requirements

Linux jobs select the public-enabled runner group `e2e-worker` with `[build-only, Linux, X64]` labels. Docker image builds and Linux job containers require a working Docker CLI and daemon on the selected worker. Changing the runner group does not install Docker.

The group currently has one Linux X64 worker. Verify Docker support with a same-head CI run before merging. Do not remove test gates or change platform labels to work around missing worker capabilities.

The Windows release row retains its existing `default` runner group and `[build-only-windows, Windows, X64]` labels. A native Windows worker with Rustup, MSVC/Windows SDK, CMake and Perl is required; the Linux worker cannot execute this row. Release publication still waits for both platform builds. Tag-only release paths are not validated by a pull-request run.
