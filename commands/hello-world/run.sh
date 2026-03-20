#!/bin/sh
# Hello World - jterm command protocol demo
#
# This script demonstrates the stdio JSON protocol by:
# 1. Showing a fuzzy-select list of greetings
# 2. Reading the user's selection
# 3. Showing an info message
# 4. Finishing with a done message (+ macOS notification)

# Step 1: Present a fuzzy selection list
cat <<'JSON'
{"type":"fuzzy","prompt":"Choose a greeting","items":[{"value":"hello","label":"Hello","description":"A classic greeting","icon":"hand.wave"},{"value":"konnichiwa","label":"こんにちは","description":"Japanese greeting","icon":"globe.asia.australia"},{"value":"bonjour","label":"Bonjour","description":"French greeting","icon":"globe.europe.africa"},{"value":"hola","label":"Hola","description":"Spanish greeting","icon":"globe.americas"}],"preview":false}
JSON

# Step 2: Read the user's response (JSON line on stdin)
read -r response

# Extract the selected value (simple approach without jq)
selected=$(echo "$response" | sed 's/.*"value":"\([^"]*\)".*/\1/')

# Step 3: Show progress
cat <<JSON
{"type":"info","message":"You selected: ${selected}"}
JSON

# Small pause so the info message is visible
sleep 0.5

# Step 4: Done with notification
cat <<JSON
{"type":"done","notify":"Greeting selected: ${selected}"}
JSON
