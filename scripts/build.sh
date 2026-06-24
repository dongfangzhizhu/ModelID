#!/bin/bash

##############################################################################
# Build script for ModelID Rust projects
# Supports Linux and macOS environments
# Usage:
#   ./scripts/build.sh              # Build all crates
#   ./scripts/build.sh modeld-core  # Build specific crate
#   ./scripts/build.sh --help       # Show help
##############################################################################

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Available crates
CRATES=(
    "modeld-core"
    "modeld-cli"
    "modeld-proxy"
    "modeld-client"
    "modeld-webui"
)

# Default values
BUILD_TYPE="debug"
RELEASE_MODE=false
TARGET_CRATES=()
VERBOSE=false

# Print help
print_help() {
    cat << EOF
${BLUE}Usage:${NC} build.sh [OPTIONS] [CRATE_NAME]

${BLUE}Options:${NC}
    -r, --release           Build in release mode (optimized)
    -v, --verbose           Show detailed build output
    -c, --check             Only check code without building
    -t, --test              Build and run tests
    -a, --all               Build all crates (default if no crate specified)
    -h, --help              Show this help message

${BLUE}Crate Names:${NC}
$(for crate in "${CRATES[@]}"; do echo "    - $crate"; done)

${BLUE}Examples:${NC}
    # Build all crates in debug mode
    ./scripts/build.sh

    # Build specific crate in release mode
    ./scripts/build.sh --release modeld-core

    # Build and test
    ./scripts/build.sh --test

    # Only check without building
    ./scripts/build.sh --check

EOF
}

# Print colored output
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Check if crate name is valid
is_valid_crate() {
    local crate=$1
    for valid_crate in "${CRATES[@]}"; do
        if [[ "$valid_crate" == "$crate" ]]; then
            return 0
        fi
    done
    return 1
}

# Build crate
build_crate() {
    local crate=$1
    local cargo_args=()

    if [[ "$VERBOSE" == true ]]; then
        cargo_args+=("--verbose")
    fi

    if [[ "$RELEASE_MODE" == true ]]; then
        cargo_args+=("--release")
    fi

    print_info "Building $crate..."
    cargo build -p "$crate" "${cargo_args[@]}"
    
    if [ $? -eq 0 ]; then
        print_success "Built $crate successfully"
    else
        print_error "Failed to build $crate"
        return 1
    fi
}

# Check crate
check_crate() {
    local crate=$1
    local cargo_args=()

    if [[ "$VERBOSE" == true ]]; then
        cargo_args+=("--verbose")
    fi

    if [[ "$RELEASE_MODE" == true ]]; then
        cargo_args+=("--release")
    fi

    print_info "Checking $crate..."
    cargo check -p "$crate" "${cargo_args[@]}"
    
    if [ $? -eq 0 ]; then
        print_success "Checked $crate successfully"
    else
        print_error "Check failed for $crate"
        return 1
    fi
}

# Test crate
test_crate() {
    local crate=$1
    local cargo_args=()

    if [[ "$VERBOSE" == true ]]; then
        cargo_args+=("--verbose")
    fi

    print_info "Testing $crate..."
    cargo test -p "$crate" "${cargo_args[@]}"
    
    if [ $? -eq 0 ]; then
        print_success "Tested $crate successfully"
    else
        print_error "Tests failed for $crate"
        return 1
    fi
}

# Main script
main() {
    # Parse arguments
    local action="build"
    
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                print_help
                exit 0
                ;;
            -r|--release)
                RELEASE_MODE=true
                BUILD_TYPE="release"
                shift
                ;;
            -v|--verbose)
                VERBOSE=true
                shift
                ;;
            -c|--check)
                action="check"
                shift
                ;;
            -t|--test)
                action="test"
                shift
                ;;
            -a|--all)
                TARGET_CRATES=("${CRATES[@]}")
                shift
                ;;
            -*)
                print_error "Unknown option: $1"
                print_help
                exit 1
                ;;
            *)
                if is_valid_crate "$1"; then
                    TARGET_CRATES+=("$1")
                else
                    print_error "Unknown crate: $1"
                    print_help
                    exit 1
                fi
                shift
                ;;
        esac
    done

    # If no crates specified, build all
    if [ ${#TARGET_CRATES[@]} -eq 0 ]; then
        TARGET_CRATES=("${CRATES[@]}")
    fi

    print_info "Starting build process..."
    print_info "Build type: $BUILD_TYPE"
    print_info "Crates to build: ${TARGET_CRATES[*]}"
    echo ""

    local failed_crates=()
    local successful_crates=()

    # Build/check/test each crate
    for crate in "${TARGET_CRATES[@]}"; do
        case $action in
            check)
                check_crate "$crate"
                ;;
            test)
                test_crate "$crate"
                ;;
            *)
                build_crate "$crate"
                ;;
        esac

        if [ $? -eq 0 ]; then
            successful_crates+=("$crate")
        else
            failed_crates+=("$crate")
        fi
        echo ""
    done

    # Print summary
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    print_info "Build Summary"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    if [ ${#successful_crates[@]} -gt 0 ]; then
        print_success "Successful: ${#successful_crates[@]} crate(s)"
        for crate in "${successful_crates[@]}"; do
            echo "  ✓ $crate"
        done
    fi

    if [ ${#failed_crates[@]} -gt 0 ]; then
        print_error "Failed: ${#failed_crates[@]} crate(s)"
        for crate in "${failed_crates[@]}"; do
            echo "  ✗ $crate"
        done
        echo ""
        exit 1
    fi

    print_success "All crates completed successfully!"
    echo ""
}

# Run main function
main "$@"
