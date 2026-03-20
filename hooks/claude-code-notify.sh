#!/usr/bin/env bash
# Claude Code notification hook for termojinal
#
# Install:
#   cp hooks/claude-code-notify.sh ~/.claude/hooks/
#   chmod +x ~/.claude/hooks/claude-code-notify.sh
#
# Then register in ~/.claude/settings.json (see hooks/claude-code-settings.example.json).

# Read JSON from stdin
input=$(cat)

# Parse event info
hook_event=$(echo "$input" | jq -r '.hook_event_name // empty')
message=$(echo "$input" | jq -r '.message // empty')
title=$(echo "$input" | jq -r '.title // "Claude Code"')
notif_type=$(echo "$input" | jq -r '.notification_type // empty')

# Only forward Notification events
if [ "$hook_event" != "Notification" ]; then
    exit 0
fi

# Build arguments
args=()
if [ -n "$title" ]; then
    args+=(--title "$title")
fi
if [ -n "$message" ]; then
    args+=(--body "$message")
fi
if [ -n "$notif_type" ]; then
    args+=(--notification-type "$notif_type")
fi

# Forward to termojinal
exec tm notify "${args[@]}"
