#!/bin/sh
# A fake agent provider with deterministic screenshot fixtures. Normal preview
# use defaults to a live stream so working and idle transitions remain visible.
fixture="${DUX_FAKE_FIXTURE:-live}"

case "$fixture" in
  steady)
    printf '%s\n' \
      'Review complete. The retry path is covered.' \
      'Files checked: src/client.rs, src/retry.rs, tests/retry.rs' \
      'Waiting for the next instruction.'
    while :; do sleep 60; done
    ;;
  working)
    printf '%s\n' \
      'Implementing bounded retries for failed API requests.' \
      '✓ Read the existing request flow' \
      '✓ Added the retry policy' \
      '→ Running focused tests'
    i=1
    while :; do
      printf '  test batch %02d passed\n' "$i"
      i=$((i + 1))
      sleep 1
    done
    ;;
  attention)
    printf '%s\n' 'Implementation is ready for review.'
    printf '\033]9;Review requested for the retry policy\007'
    while :; do sleep 60; done
    ;;
  failure)
    printf '%s\n' 'Error: the fixture dependency could not be resolved.' >&2
    exit 2
    ;;
  live)
    i=0
    echo "fake-agent: streaming output so this session reads as Working."
    echo "fake-agent: close this tab or interrupt to return the agent to Idle."
    while true; do
      printf 'fake-agent working... line %d @ %s\n' "$i" "$(date +%H:%M:%S)"
      i=$((i + 1))
      sleep 0.6
    done
    ;;
  *)
    echo "fake-agent: unknown fixture: $fixture" >&2
    exit 64
    ;;
esac
