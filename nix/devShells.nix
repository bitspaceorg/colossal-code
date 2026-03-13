{ ... }:
{
    perSystem =
        {
            pkgs,
            self',
            cocode-deps,
            ...
        }:
        {
            devShells.default = pkgs.mkShell {
                nativeBuildInputs = cocode-deps.nativeBuildInputs;
                buildInputs = cocode-deps.buildInputs;
                inputsFrom = [
                    self'.devShells.treefmt
                    self'.devShells.precommit
                ];
                shellHook = ''
                    echo "Colossal Code — development shell"
                    echo "  Check:     cargo check"
                    echo "  Test:      cargo test"
                    echo "  Lint:      cargo clippy --all-targets --no-deps -- -D warnings"
                    echo "  Format:    treefmt"
                '';
            };
        };
}
