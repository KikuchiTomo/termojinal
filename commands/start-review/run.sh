#!/usr/bin/env bash
set -euo pipefail

# Start PR Review - select a PR awaiting your review, create a worktree, and
# prepare it for review.

# Step 1: List PRs awaiting review via gh CLI
prs=$(gh pr list --search "review-requested:@me" --json number,title,headRefName,author --jq '.[] | "\(.number)\t\(.title)\t\(.headRefName)\t\(.author.login)"')

if [ -z "$prs" ]; then
    echo '{"type":"info","message":"No PRs awaiting your review"}'
    sleep 1
    echo '{"type":"done"}'
    exit 0
fi

# Build fuzzy items from PR list
items="["
first=true
while IFS=$'\t' read -r num title branch author; do
    if [ "$first" = true ]; then first=false; else items+=","; fi
    items+="{\"value\":\"$num\",\"label\":\"#$num $title\",\"description\":\"$author → $branch\"}"
done <<< "$prs"
items+="]"

echo "{\"type\":\"fuzzy\",\"prompt\":\"Select PR to review\",\"items\":$items}"

# Step 2: Read user selection
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

selected=$(echo "$response" | jq -r '.value // empty')
if [ -z "$selected" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Step 3: Set up worktree for the selected PR
echo '{"type":"info","message":"Setting up worktree..."}'
branch=$(gh pr view "$selected" --json headRefName --jq '.headRefName')
git fetch origin "$branch"

worktree_dir="../review-pr-$selected"
if [ ! -d "$worktree_dir" ]; then
    git worktree add "$worktree_dir" "origin/$branch"
fi

echo "{\"type\":\"done\",\"notify\":\"PR #$selected ready for review in $worktree_dir\"}"
