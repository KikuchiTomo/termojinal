#!/usr/bin/env bash
set -euo pipefail

# Clone & Open - clone a git repository and open it in a new session.

# Step 1: Ask for the repository URL
echo '{"type":"text","label":"Repository URL","placeholder":"https://github.com/user/repo.git","default":""}'
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

url=$(echo "$response" | jq -r '.value // empty')
if [ -z "$url" ]; then
    echo '{"type":"done"}'
    exit 0
fi

# Extract repo name from URL
repo_name=$(basename "$url" .git)

# Step 2: Ask for clone directory
echo '{"type":"text","label":"Clone directory","placeholder":"~/repos","default":"~/repos"}'
read -r response

# Handle cancellation
type=$(echo "$response" | jq -r '.type // empty')
if [ "$type" = "cancelled" ]; then
    echo '{"type":"done"}'
    exit 0
fi

clone_dir=$(echo "$response" | jq -r '.value // empty')
clone_dir="${clone_dir/#\~/$HOME}"

if [ -z "$clone_dir" ]; then
    clone_dir="$HOME/repos"
fi

target="$clone_dir/$repo_name"

# Step 3: Clone or confirm opening existing directory
if [ -d "$target" ]; then
    echo '{"type":"confirm","message":"Directory already exists. Open it anyway?","default":true}'
    read -r confirm

    type=$(echo "$confirm" | jq -r '.type // empty')
    if [ "$type" = "cancelled" ]; then
        echo '{"type":"done"}'
        exit 0
    fi

    confirmed=$(echo "$confirm" | jq -r '.yes')
    if [ "$confirmed" != "true" ]; then
        echo '{"type":"done"}'
        exit 0
    fi
else
    echo "{\"type\":\"info\",\"message\":\"Cloning $url...\"}"
    mkdir -p "$clone_dir"
    git clone "$url" "$target" 2>/dev/null
fi

echo "{\"type\":\"done\",\"notify\":\"Cloned $repo_name to $target\"}"
