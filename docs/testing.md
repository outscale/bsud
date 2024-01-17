# Test Coverage

Is bsud fully tested?

![maybe-test-are-complete](maybe.gif)

# Basic Check

```bash
cargo clippy
```

# E2E Tests

E2E tests run real life scenario on actual platform, attaching real BSUs, making I/Os, growing, shrinking, stressing, ...

This mean you will need the following pre-requisites to run those tests:
- Run tests on one Outscale's VM.
- Have a pair of [Access Key and Secret Key](https://docs.outscale.com/en/userguide/About-Access-Keys.html) ready.
- Some disk quota (gp2 and io1)
- bsud binary
- Some software installed:
    - curl >= v7.75

E2E tests take time as they will actually write and read real data do drives.

## Running E2E Tests

Set credentials and test log levels. You may need `RUST_LOG=bsud_test=trace,bsudlib=trace` level of details to debug your tests.
```bash
export OSC_ACCESS_KEY=XXX
export OSC_SECRET_KEY=YYY
export RUST_LOG=off
```

Build & run all tests:
```bash
find ./target/debug/deps/ -type f -regex '.*bsud_tests-[a-z0-9]+' -exec rm ./{} \; &&\
cargo test --no-run &&\
time sudo bash -c "find ./target/debug/deps/ -type f -regex '.*bsud_tests-[a-z0-9]+' | RUST_LOG=$RUST_LOG OSC_ACCESS_KEY=$OSC_ACCESS_KEY OSC_SECRET_KEY=$OSC_SECRET_KEY xargs -i sh -c \"./{} --fail-fast --concurrency 1\"" 2> logs.txt
```

This will:
1. delete any compiled tests
2. build tests as current user
3. find compiled test and run it as root with some test options

Note: We don't want to build as root as file rights will be messed up and there is no simple way yet to just run tests without building for now.

Tips:
- If you want to run a specific feature file, add `--input basic-lifecycle.feature` (after `--fail-fast`).
- Remove `--concurrency` option to avoid the limit. Test output will look weird as all tests are running in parallel but this should be faster.

## Developping E2E tests

Tests are based on Behavior-Based Driven using [cucumber-rs](https://cucumber-rs.github.io/cucumber/current/) with [Gherkin](https://cucumber.io/docs/gherkin/reference/) syntax.

All features are located in [/tests/features](./features/) folder.

# Cleaning

When successful, test drives are removed but in case of failure some drive could remain.

Use `./tests/delete-drive.sh` to manually remove drives