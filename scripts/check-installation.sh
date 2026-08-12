#!/bin/bash
# contextzip Installation Verification Script
# Helps diagnose if contextzip is correctly installed.

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "==========================================================="
echo "       contextzip Installation Verification"
echo "==========================================================="
echo ""

# Check 1: contextzip installed?
echo "1. Checking if contextzip is installed..."
if command -v contextzip &> /dev/null; then
    echo -e "   ${GREEN}contextzip is installed${NC}"
    CZ_PATH=$(which contextzip)
    echo "   Location: $CZ_PATH"
else
    echo -e "   ${RED}contextzip is NOT installed${NC}"
    echo ""
    echo "   Install with:"
    echo "   cargo install --path ."
    exit 1
fi
echo ""

# Check 2: contextzip version
echo "2. Checking contextzip version..."
CZ_VERSION=$(contextzip --version 2>/dev/null || echo "unknown")
echo "   Version: $CZ_VERSION"
echo ""

# Check 3: gain subcommand works (distinguishes contextzip from other binaries)
echo "3. Verifying gain subcommand is available..."
if contextzip gain &>/dev/null || contextzip gain --help &>/dev/null; then
    echo -e "   ${GREEN}CORRECT - contextzip gain works${NC}"
    CORRECT_CZ=true
else
    echo -e "   ${RED}gain subcommand not found - binary may be wrong or outdated${NC}"
    echo ""
    echo "   Reinstall with:"
    echo "   cargo install --path . --force"
    CORRECT_CZ=false
fi
echo ""

if [ "$CORRECT_CZ" = false ]; then
    echo "==========================================================="
    echo -e "${RED}INSTALLATION CHECK FAILED${NC}"
    echo "==========================================================="
    exit 1
fi

# Check 4: Available features
echo "4. Checking available features..."
FEATURES=()
MISSING_FEATURES=()

check_command() {
    local cmd=$1
    local name=$2
    if contextzip --help 2>/dev/null | grep -qw "$cmd"; then
        echo -e "   ${GREEN}ok${NC} $name"
        FEATURES+=("$name")
    else
        echo -e "   ${YELLOW}missing${NC}  $name (upgrade?)"
        MISSING_FEATURES+=("$name")
    fi
}

check_command "gain" "Token savings analytics"
check_command "git" "Git operations"
check_command "gh" "GitHub CLI"
check_command "pnpm" "pnpm support"
check_command "vitest" "Vitest test runner"
check_command "lint" "ESLint/linters"
check_command "tsc" "TypeScript compiler"
check_command "next" "Next.js"
check_command "prettier" "Prettier"
check_command "playwright" "Playwright E2E"
check_command "prisma" "Prisma ORM"
check_command "discover" "Discover missed savings"

echo ""

# Check 5: CLAUDE.md initialization
echo "5. Checking Claude Code integration..."
GLOBAL_INIT=false
LOCAL_INIT=false

if [ -f "$HOME/.claude/CLAUDE.md" ] && grep -q "contextzip" "$HOME/.claude/CLAUDE.md"; then
    echo -e "   ${GREEN}ok${NC} Global CLAUDE.md initialized (~/.claude/CLAUDE.md)"
    GLOBAL_INIT=true
else
    echo -e "   ${YELLOW}warn${NC}  Global CLAUDE.md not initialized"
    echo "      Run: contextzip init --global"
fi

if [ -f "./CLAUDE.md" ] && grep -q "contextzip" "./CLAUDE.md"; then
    echo -e "   ${GREEN}ok${NC} Local CLAUDE.md initialized (./CLAUDE.md)"
    LOCAL_INIT=true
else
    echo -e "   ${YELLOW}warn${NC}  Local CLAUDE.md not initialized in current directory"
    echo "      Run: contextzip init (in your project directory)"
fi
echo ""

# Check 6: Auto-rewrite hook
echo "6. Checking auto-rewrite hook (optional but recommended)..."
if [ -f "$HOME/.claude/hooks/contextzip-rewrite.sh" ]; then
    echo -e "   ${GREEN}ok${NC} Hook script installed"
    if [ -f "$HOME/.claude/settings.json" ] && grep -q "contextzip-rewrite.sh" "$HOME/.claude/settings.json"; then
        echo -e "   ${GREEN}ok${NC} Hook enabled in settings.json"
    else
        echo -e "   ${YELLOW}warn${NC}  Hook script exists but not enabled in settings.json"
        echo "      See README.md 'Auto-Rewrite Hook' section"
    fi
else
    echo -e "   ${YELLOW}warn${NC}  Auto-rewrite hook not installed (optional)"
    echo "      Install: cp .claude/hooks/contextzip-rewrite.sh ~/.claude/hooks/"
fi
echo ""

# Summary
echo "==========================================================="
echo "                    SUMMARY"
echo "==========================================================="

if [ ${#MISSING_FEATURES[@]} -gt 0 ]; then
    echo -e "${YELLOW}You have a partial contextzip installation${NC}"
    echo ""
    echo "Missing features:"
    for feature in "${MISSING_FEATURES[@]}"; do
        echo "  - $feature"
    done
    echo ""
    echo "To rebuild with all features:"
    echo "  cargo install --path . --force"
else
    echo -e "${GREEN}Full-featured contextzip installation detected${NC}"
fi

echo ""

if [ "$GLOBAL_INIT" = false ] && [ "$LOCAL_INIT" = false ]; then
    echo -e "${YELLOW}contextzip not initialized for Claude Code${NC}"
    echo "   Run: contextzip init --global (for all projects)"
    echo "   Or:  contextzip init (for this project only)"
fi

echo ""
echo "Need help? See docs/TROUBLESHOOTING.md"
echo "==========================================================="
