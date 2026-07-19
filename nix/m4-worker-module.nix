{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.tempestmikuM4Worker;
  linkedFolderType = lib.types.submodule {
    options = {
      name = lib.mkOption { type = lib.types.str; };
      path = lib.mkOption { type = lib.types.path; };
      mode = lib.mkOption {
        type = lib.types.enum [
          "ro"
          "rw"
        ];
        default = "rw";
      };
      commands = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
      };
    };
  };
  hostConfig = pkgs.writeText "tempestmiku-m4-worker-host.json" (
    builtins.toJSON {
      linked_folders = map (folder: {
        inherit (folder)
          name
          path
          mode
          commands
          ;
        safe_args = [ ];
      }) cfg.linkedFolders;
      approvals = {
        mode = "deny";
        timeout_ms = cfg.approvalTimeoutMs;
      };
      artifact_root = "${cfg.stateRoot}/artifacts";
      proc_run_timeout_ms = cfg.procRunTimeoutMs;
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
  credentialPath = "/run/credentials/${cfg.unitName}.service/signing-key";
  workerConfig = pkgs.writeText "tempestmiku-m4-worker.json" (
    builtins.toJSON {
      workerId = cfg.workerId;
      listenAddr = cfg.listenAddress;
      signingKeyFile = credentialPath;
      hostConfigFile = hostConfig;
      ledgerRoot = cfg.stateRoot;
      approvalTimeoutMs = cfg.approvalTimeoutMs;
      maxConcurrentJobs = 4;
      maxConcurrentProcRuns = 1;
      retentionSeconds = 86400;
    }
  );
  launchWorker = pkgs.writeShellScript "tempestmiku-m4-worker-launch" ''
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
    exec ${cfg.package}/bin/tm-worker
  '';
in
{
  options.services.tempestmikuM4Worker = {
    enable = lib.mkEnableOption "the TempestMiku M4 hardened linked-host worker";
    package = lib.mkOption {
      type = lib.types.package;
      description = "tm-worker package.";
    };
    isolationRuntime = lib.mkOption {
      type = lib.types.package;
      description = "Root-owned immutable bubblewrap and static command runtime.";
    };
    signingKeyFile = lib.mkOption {
      type = lib.types.path;
      description = "Root-readable file containing the 64-character lowercase hex HMAC key.";
    };
    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:18787";
    };
    workerId = lib.mkOption {
      type = lib.types.str;
      default = "homolab-m4";
    };
    linkedFolders = lib.mkOption {
      type = lib.types.listOf linkedFolderType;
      default = [
        {
          name = "tempestmiku";
          path = "${cfg.stateRoot}/linked/tempestmiku";
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
        }
      ];
    };
    approvalTimeoutMs = lib.mkOption {
      type = lib.types.ints.between 1000 300000;
      default = 60000;
    };
    procRunTimeoutMs = lib.mkOption {
      type = lib.types.ints.between 1 900000;
      default = 180000;
    };
    unitName = lib.mkOption {
      type = lib.types.str;
      default = "tempestmiku-m4-worker";
      readOnly = true;
    };
    user = lib.mkOption {
      type = lib.types.str;
      default = "tempestmiku-worker";
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
      default = "/var/lib/tempestmiku-worker";
      readOnly = true;
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isLinux;
        message = "services.tempestmikuM4Worker requires Linux";
      }
      {
        assertion = cfg.linkedFolders != [ ];
        message = "services.tempestmikuM4Worker requires at least one linked folder";
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
      "d ${cfg.stateRoot}/jobs 0700 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateRoot}/linked 0750 root root -"
    ]
    ++ map (folder: "d ${folder.path} 0700 ${cfg.user} ${cfg.user} -") cfg.linkedFolders;

    systemd.services.${cfg.unitName} = {
      description = "TempestMiku M4 hardened linked-host worker";
      wantedBy = [ "multi-user.target" ];
      after = [
        "network.target"
        "tailscaled.service"
      ];
      wants = [ "tailscaled.service" ];
      environment = {
        TM_WORKER_CONFIG = workerConfig;
      };
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.user;
        ExecStart = launchWorker;
        Restart = "on-failure";
        RestartSec = "2s";
        Delegate = "cpu memory pids";
        DelegateSubgroup = "service";
        LoadCredential = "signing-key:${cfg.signingKeyFile}";
        StateDirectory = "tempestmiku-worker";
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

    environment.etc."tempestmiku/m4-worker-host.json".source = hostConfig;
  };
}
