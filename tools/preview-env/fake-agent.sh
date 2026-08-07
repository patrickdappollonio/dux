#!/bin/sh
# A fake agent provider: streams output forever so dux sees a continuously
# "working" PTY. This is the only reliable way to exercise the working-state
# visuals (the sidebar bob + name shimmer) without authenticating a real agent,
# whose login screen renders once and then sits idle (not "working").
#
# Configured in the seeded config as [providers.fake]; pick it in the New-agent
# picker to watch an agent go green/bob. Ctrl-C in its tab (or closing it) stops
# the stream, dropping the agent back to Idle so both transitions are testable.
i=0
echo "fake-agent: streaming output so this session reads as Working."
echo "fake-agent: close this tab or interrupt to return the agent to Idle."
while true; do
  printf 'fake-agent working... line %d @ %s\n' "$i" "$(date +%H:%M:%S)"
  i=$((i + 1))
  sleep 0.6
done
