{
  description = "Antigravity Usage Monitor (AUM) — lightweight TUI & Waybar usage monitor for NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      mkPackage = pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "antigravity-usage-monitor";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.sqlite ];
          meta = with pkgs.lib; {
            description = "Lightweight terminal dashboard and Waybar module for Google Antigravity CLI usage";
            license = licenses.mit;
            mainProgram = "agmonitor";
            platforms = platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          pkg = mkPackage pkgs;
        in
        {
          default = pkg;
          agmonitor = pkg;
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.agmonitor}/bin/agmonitor";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustc
              cargo
              rust-analyzer
              rustfmt
              clippy
              pkg-config
              sqlite
            ];
          };
        });

      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.antigravity-usage-monitor;
          defaultPkg = self.packages.${pkgs.stdenv.hostPlatform.system}.agmonitor;
        in
        {
          options.services.antigravity-usage-monitor = {
            enable = lib.mkEnableOption "Antigravity Usage Monitor system package and optional user refresh timer";
            package = lib.mkOption {
              type = lib.types.package;
              default = defaultPkg;
              description = "The agmonitor package to install and run.";
            };
            enableUserTimer = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Enable systemd --user timer to refresh agmonitor cache every 5 minutes.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];

            systemd.user.services.agmonitor-refresh = lib.mkIf cfg.enableUserTimer {
              description = "Refresh Antigravity Usage Monitor cache and SQLite history";
              after = [ "graphical-session.target" ];
              serviceConfig = {
                Type = "oneshot";
                ExecStart = "${cfg.package}/bin/agmonitor refresh --signal-waybar";
              };
            };

            systemd.user.timers.agmonitor-refresh = lib.mkIf cfg.enableUserTimer {
              description = "Periodic Antigravity Usage Monitor refresh";
              wantedBy = [ "timers.target" ];
              timerConfig = {
                OnBootSec = "30s";
                OnUnitActiveSec = "5m";
              };
            };
          };
        };

      homeManagerModules.default = { pkgs, ... }@args:
        let
          pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.agmonitor;
        in
        import ./nix/hm-module.nix { agmonitorPackage = pkg; } args;
    };
}
