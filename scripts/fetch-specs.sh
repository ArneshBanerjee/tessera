#!/bin/sh
# Downloads Atlassian's public OpenAPI specs into spec/.
# Tessera is built clean-room from these public documents only.
set -eu
cd "$(dirname "$0")/.."
mkdir -p spec
fetch() { curl -fsSL -A "Mozilla/5.0" -o "spec/$1" "$2"; echo "spec/$1"; }
fetch jira-platform-v3.json https://developer.atlassian.com/cloud/jira/platform/swagger-v3.v3.json
fetch jira-software.json https://developer.atlassian.com/cloud/jira/software/swagger.v3.json
fetch confluence-v2.json https://dac-static.atlassian.com/cloud/confluence/openapi-v2.v3.json
