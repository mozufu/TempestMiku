{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.tempestmikuM4;
  hostConfig = pkgs.writeText "tempestmiku-m4-host.json" (
    builtins.toJSON {
      linked_folders = [
        {
          name = "tempestmiku";
          path = cfg.linkedRoot;
          mode = "rw";
          commands = [
            "cat"
            "env"
            "mount"
            "resource-probe"
            "sh"
            "sleep"
            "test"
            "thread-probe"
            "touch"
            "unshare"
            "wget"
          ];
          safe_args = [ ];
        }
      ];
      approvals = {
        mode = "deny";
        timeout_ms = 60000;
      };
      artifact_root = "${cfg.stateRoot}/artifacts";
      proc_run_timeout_ms = 180000;
      proc_isolation = {
        provider = "linux_hardened_v1";
        launcher = "${cfg.isolationRuntime}/bin/bwrap";
        runtime_roots = [ "${cfg.isolationRuntime}" ];
        limits = {
          address_space_bytes = 2147483648;
          process_count = 64;
          open_files = 256;
        };
        cgroup_root = "/sys/fs/cgroup/system.slice/${cfg.unitName}.service";
        cgroup_limits = {
          memory_max_bytes = 1073741824;
          memory_swap_max_bytes = 0;
          pids_max = 64;
          cpu_quota_micros = 100000;
          cpu_period_micros = 100000;
        };
      };
    }
  );
  launchServer = pkgs.writeShellScript "tempestmiku-m4-launch" ''
    set -euo pipefail

    cgroup_root=/sys/fs/cgroup/system.slice/${cfg.unitName}.service
    test -d "$cgroup_root/service"
    if test "$(< /proc/self/cgroup)" != "0::/system.slice/${cfg.unitName}.service/service"; then
      printf 'main process is not inside the delegated service subgroup\n' >&2
      exit 1
    fi
    if test -s "$cgroup_root/cgroup.procs"; then
      printf 'delegated cgroup root still has resident processes\n' >&2
      exit 1
    fi
    available=" $(<"$cgroup_root/cgroup.controllers") "
    for controller in cpu memory pids; do
      case "$available" in
        *" $controller "*) ;;
        *)
          printf 'required cgroup controller is unavailable: %s\n' "$controller" >&2
          exit 1
          ;;
      esac
    done
    printf '+cpu +memory +pids\n' > "$cgroup_root/cgroup.subtree_control"
    enabled=" $(<"$cgroup_root/cgroup.subtree_control") "
    for controller in cpu memory pids; do
      case "$enabled" in
        *" $controller "*) ;;
        *)
          printf 'required cgroup controller was not enabled: %s\n' "$controller" >&2
          exit 1
          ;;
      esac
    done
    exec ${cfg.package}/bin/tm-server
  '';
in
{
  options.services.tempestmikuM4 = {
    enable = lib.mkEnableOption "the TempestMiku M4 production hardening target";
    package = lib.mkOption {
      type = lib.types.package;
      description = "tm-server package used by the production hardening target.";
    };
    isolationRuntime = lib.mkOption {
      type = lib.types.package;
      description = "Root-owned immutable bubblewrap and static command runtime.";
    };
    unitName = lib.mkOption {
      type = lib.types.str;
      default = "tempestmiku-m4";
      readOnly = true;
    };
    user = lib.mkOption {
      type = lib.types.str;
      default = "tempestmiku-m4";
      readOnly = true;
    };
    uid = lib.mkOption {
      type = lib.types.int;
      default = 23017;
    };
    gid = lib.mkOption {
      type = lib.types.int;
      default = 23017;
    };
    stateRoot = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/tempestmiku-m4";
      readOnly = true;
    };
    linkedRoot = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/tempestmiku-m4/linked";
      readOnly = true;
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isLinux;
        message = "services.tempestmikuM4 requires Linux";
      }
    ];

    users.groups.${cfg.user}.gid = cfg.gid;
    users.users.${cfg.user} = {
      isSystemUser = true;
      uid = cfg.uid;
      group = cfg.user;
      home = cfg.stateRoot;
    };

    systemd.tmpfiles.rules = [
      "d ${cfg.stateRoot} 0750 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateRoot}/artifacts 0700 ${cfg.user} ${cfg.user} -"
      "d ${cfg.linkedRoot} 0700 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateRoot}/acceptance 0700 ${cfg.user} ${cfg.user} -"
    ];

    systemd.services.${cfg.unitName} = {
      description = "TempestMiku M4 production hardening target";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      environment = {
        TM_HOST_CONFIG = hostConfig;
        TM_SERVER_ADDR = "127.0.0.1:8787";
        TM_SERVER_ROLE = "api";
      };
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.user;
        ExecStart = launchServer;
        Restart = "on-failure";
        RestartSec = "2s";
        Delegate = "cpu memory pids";
        DelegateSubgroup = "service";
        StateDirectory = "tempestmiku-m4";
        StateDirectoryMode = "0750";
        UMask = "0077";
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateTmp = true;
        ProtectClock = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.stateRoot ];
        RestrictRealtime = true;
        LockPersonality = true;
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
      };
    };

    environment.etc."tempestmiku/m4-host.json".source = hostConfig;
  };
}
