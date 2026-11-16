{ agmonitorPackage }:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.antigravity-usage-monitor;
  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.antigravity-usage-monitor = {
    enable = lib.mkEnableOption "Antigravity Usage Monitor (agmonitor)";

    package = lib.mkOption {
      type = lib.types.package;
      default = agmonitorPackage;
      description = "The agmonitor package to install.";
    };

    settings = lib.mkOption {
      type = tomlFormat.type;
      default = { };
      description = "Settings written to ~/.config/agmonitor/config.toml.";
      example = lib.literalExpression ''
        {
          general = {
            refresh_interval_secs = 300;
            mask_email = true;
          };
          waybar = {
            display_mode = "tokens_5h";
            signal = 10;
          };
        }
      '';
    };

    systemdTimer = {
      enable = lib.mkEnableOption "periodic background refresh systemd user timer";
      interval = lib.mkOption {
        type = lib.types.str;
        default = "5m";
        description = "Systemd OnUnitActiveSec interval for refreshing snapshot & history DB.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."agmonitor/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "agmonitor-config.toml" cfg.settings;
    };

    systemd.user.services.agmonitor-refresh = lib.mkIf cfg.systemdTimer.enable {
      Unit = {
        Description = "Refresh Antigravity Usage Monitor cache and SQLite history";
        After = [ "network-online.target" ];
      };
      Service = {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/agmonitor refresh --signal-waybar";
      };
    };

    systemd.user.timers.agmonitor-refresh = lib.mkIf cfg.systemdTimer.enable {
      Unit = {
        Description = "Periodic Antigravity Usage Monitor refresh timer";
      };
      Timer = {
        OnBootSec = "30s";
        OnUnitActiveSec = cfg.systemdTimer.interval;
        Unit = "agmonitor-refresh.service";
      };
      Install = {
        WantedBy = [ "timers.target" ];
      };
    };
  };
}
