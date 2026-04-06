{ inputs, ... }:
{
    imports = [ inputs.treefmt.flakeModule ];

    perSystem =
        { config, pkgs, ... }:
        {
            treefmt.config = {
                projectRootFile = "flake.nix";
                flakeCheck = false;
                settings.global.excludes = [
                    "docs/data.json"
                    "flake.lock"
                    "target"
                    "external/**"
                ];
                package = pkgs.treefmt;

                programs = {
                    rustfmt = {
                        enable = true;
                        includes = [ "**/*.rs" ];
                    };

                    nixfmt = {
                        enable = true;
                        strict = true;
                        width = 160;
                        indent = 4;
                    };

                    prettier = {
                        enable = true;
                        includes = [
                            "**/*.md"
                            "**/*.mdx"
                            "**/*.yml"
                            "**/*.yaml"
                            "**/*.json"
                        ];
                        excludes = [
                            "target"
                            "result"
                            "result-*"
                        ];
                    };

                    shfmt = {
                        enable = true;
                        indent_size = 4;
                        simplify = true;
                    };
                };

                settings.formatter.prettier.options = [
                    "--print-width"
                    "100"
                    "--tab-width"
                    "4"
                    "--trailing-comma"
                    "es5"
                    "--end-of-line"
                    "lf"
                ];
            };

            devShells.treefmt = pkgs.mkShell { buildInputs = [ config.treefmt.build.wrapper ] ++ (builtins.attrValues config.treefmt.build.programs); };
        };
}
