{ inputs, ... }:
{
    imports = [ inputs.precommit.flakeModule ];

    perSystem =
        {
            config,
            pkgs,
            lib,
            ...
        }:
        let
            python = pkgs.python3.withPackages (ps: [ ps.python-frontmatter ]);
            hook-script = pkgs.writeShellScript "docs-build-hook" ''
                set -euo pipefail
                cd "$(${pkgs.git}/bin/git rev-parse --show-toplevel)"
                ${python}/bin/python utils/docs/validate.py
                ${python}/bin/python utils/docs/build_index.py
                ${pkgs.git}/bin/git add docs/data.json
            '';
        in
        {
            pre-commit = {
                check.enable = false;

                settings = {
                    hooks = {
                        treefmt.enable = true;
                        nil.enable = lib.mkDefault true;
                        docs-build = {
                            enable = true;
                            name = "docs-build";
                            entry = "${hook-script}";
                            language = "system";
                            pass_filenames = false;
                            always_run = true;
                        };
                    };
                };
            };

            checks.dev-pre-commit = config.pre-commit.settings.run;

            devShells.precommit = pkgs.mkShell { shellHook = config.pre-commit.settings.shellHook; };
        };
}
