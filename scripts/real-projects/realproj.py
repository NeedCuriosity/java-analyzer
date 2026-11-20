#!/usr/bin/env python3
"""Real-project checks for java-analyzer. Run through run.sh, which sets up Maven, the JDKs and
the binary and exports:

  JA_REALPROJ_WORK   work directory (isolated HOME, checkouts, logs)
  JA_REALPROJ_MAVEN  Maven installation
  JA_REALPROJ_JDKS   "8=/path 11=/path ... 25=/path"; 25 becomes JAVA_HOME
  JA_BIN             java-analyzer binary

Checks (all by default, or name some on the command line):

  classpath  per module: the runtime class path, JDK and encoding from
             `java-analyzer/projectClasspath`, diffed against `mvn dependency:build-classpath`
             (run together with `compile` over the reactor);
             the JDK must match the language level (several JDKs installed, JAVA_HOME=25)
  launch     start the main class through the debug adapter, wait for Spring's "Started"
  settings   custom settings.xml whose <localRepository> is elsewhere: import must not run mvn
  gbk        GBK-encoded sources: diagnostics, goto definition into an unopened file, output
  stats      `java-analyzer analysis-stats` on every project must not panic

Optional environment: see README.md.
"""

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from jaclient import DapClient, LspClient, file_uri  # noqa: E402

WORK = pathlib.Path(os.environ["JA_REALPROJ_WORK"])
MAVEN = pathlib.Path(os.environ["JA_REALPROJ_MAVEN"])
JDKS = {int(k): pathlib.Path(v) for k, v in (e.split("=", 1) for e in os.environ["JA_REALPROJ_JDKS"].split())}
JA = os.environ["JA_BIN"]
JAVA_HOME = JDKS[25]
HOME = WORK / "home"
PROJECTS_DIR = WORK / "projects"
LOGS = WORK / "logs"
LAUNCH_TIMEOUT = int(os.environ.get("JA_LAUNCH_TIMEOUT", "90"))

# Pinned checkouts. `launch`: (module directory, main class[, extra vmArgs]).
PROJECTS = [
    dict(
        name="spring-petclinic",
        url="https://github.com/spring-projects/spring-petclinic.git",
        commit="500158f732419217507c7656904b8e6aa1bcc0d6",
        launch=(".", "org.springframework.samples.petclinic.PetClinicApplication"),
    ),
    dict(
        name="RuoYi-Vue",
        url="https://gitee.com/y_project/RuoYi-Vue.git",
        commit="ce2072b810ee642516b7795dfd52bab28c89beb2",
        # its logback.xml writes to /home/ruoyi/logs: log to the console instead
        launch=("ruoyi-admin", "com.ruoyi.RuoYiApplication",
                ["-Dlogback.configurationFile={console_logback}", "-Dlogging.config={console_logback}"]),
    ),
    dict(
        name="mall",
        url="https://gitee.com/macrozheng/mall.git",
        commit="dcaa93b3150352e5044708d7211b5bed0af4509f",
        launch=("mall-admin", "com.macro.mall.MallAdminApplication"),
    ),
    dict(
        name="eladmin",
        url="https://gitee.com/elunez/eladmin.git",
        commit="55fbf705956949697dbd68bf9003776609d3d029",
        launch=("eladmin-system", "me.zhengjie.AppRun"),
    ),
    dict(
        name="pig",
        url="https://gitee.com/log4j/pig.git",
        commit="193997016e39c2158a11c6da3ec1fc9fa3162a9d",
        launch=None,  # needs Nacos before Spring gets anywhere
    ),
    dict(
        name="commons-io-2.4",
        url="https://github.com/apache/commons-io.git",
        commit="e36d53170875d26d59ca94bd376bf40bc5690ee6",
        launch=None,
    ),
]

# Flags that keep the Maven build to compiling.
MVN_SKIP = [
    "-DskipTests",
    "-Dcheckstyle.skip",
    "-Dspring-javaformat.skip",
    "-Dmaven.javadoc.skip",
    "-Djacoco.skip",
    "-Drat.skip",
    "-Danimal.sniffer.skip",
    "-Denforcer.skip",
    "-Dmaven.source.skip",
    "-Dgpg.skip",
]

# Failures that mean the class path (or the JDK) is wrong.
LINKAGE = re.compile(
    r"\b(ClassNotFoundException|NoClassDefFoundError|NoSuchMethodError|NoSuchFieldError|"
    r"LinkageError|IncompatibleClassChangeError|AbstractMethodError|UnsupportedClassVersionError|"
    r"ClassFormatError|InaccessibleObjectException|TypeNotPresentException|"
    r"Could not find or load main class)\b"
)
# Failures caused by external services that are not running here.
SERVICES = re.compile(
    r"Communications link failure|Connection refused|Unable to connect to Redis|"
    r"CannotGetJdbcConnectionException|Failed to obtain JDBC Connection|"
    r"Failed to determine a suitable driver class|connect timed out|UnknownHostException|"
    r"Access denied for user|RedisConnectionFailureException|Unable to connect to localhost"
)
SPRING_STARTED = re.compile(r"Started \S+ in [\d.]+ seconds")
SPRING_LOADING = re.compile(r"Starting \S+ (v\S+ )?using Java|:: Spring Boot ::")


# ---------------------------------------------------------------------------------------------
# Environment


def env_for(home, extra=None):
    keep = ("PATH", "TERM", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "CARGO_HOME", "RUSTUP_HOME")
    env = {k: v for k, v in os.environ.items() if k in keep}
    env.update(
        HOME=str(home),
        XDG_CACHE_HOME=str(home / ".cache"),
        XDG_CONFIG_HOME=str(home / ".config"),
        JAVA_HOME=str(JAVA_HOME),
        MAVEN_OPTS=f"-Duser.home={home}",
        PATH=f"{MAVEN}/bin:{JAVA_HOME}/bin:{os.environ['PATH']}",
    )
    env.pop("MAVEN_HOME", None)
    env.pop("M2_HOME", None)
    env.update(extra or {})
    return env


def prepare_home(home):
    """Isolated HOME: settings.xml (optional mirror) and every JDK but JAVA_HOME in ~/.jdks."""
    (home / ".m2").mkdir(parents=True, exist_ok=True)
    mirror = os.environ.get("JA_MAVEN_MIRROR")
    mirror_id = os.environ.get("JA_MAVEN_MIRROR_ID", "mirror")
    mirrors = (
        f"<mirrors><mirror><id>{mirror_id}</id><mirrorOf>central</mirrorOf><url>{mirror}</url></mirror></mirrors>"
        if mirror
        else ""
    )
    proxy = os.environ.get("JA_MAVEN_PROXY")
    proxies = ""
    if proxy:
        host, _, port = proxy.rpartition(":")
        proxies = "<proxies>" + "".join(
            f"<proxy><id>{p}</id><active>true</active><protocol>{p}</protocol><host>{host}</host><port>{port}</port></proxy>"
            for p in ("http", "https")
        ) + "</proxies>"
    (home / ".m2" / "settings.xml").write_text(f"<settings>{mirrors}{proxies}</settings>\n")
    jdks = home / ".jdks"
    shutil.rmtree(jdks, ignore_errors=True)
    jdks.mkdir()
    for version, path in JDKS.items():
        if path != JAVA_HOME:
            (jdks / f"jdk{version}").symlink_to(path)


def run(cmd, log, cwd=None, env=None, timeout=3600):
    with open(log, "ab") as out:
        out.write(f"\n$ {' '.join(map(str, cmd))}  (in {cwd})\n".encode())
        out.flush()
        try:
            return subprocess.run(cmd, cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT, timeout=timeout).returncode
        except subprocess.TimeoutExpired:
            out.write(b"TIMEOUT\n")
            return -1


def mvn(args, log, cwd, env):
    """Run Maven, retrying twice: downloads fail now and then on a flaky network."""
    for _ in range(3):
        code = run(["mvn", "-B", "-q", *args], log, cwd=cwd, env=env)
        if code == 0:
            return 0
    return code


def checkout(project, log):
    log.parent.mkdir(parents=True, exist_ok=True)
    dest = PROJECTS_DIR / project["name"]
    if not (dest / ".git").is_dir():
        shutil.rmtree(dest, ignore_errors=True)
        dest.mkdir(parents=True)
        run(["git", "init", "-q"], log, cwd=dest)
        run(["git", "remote", "add", "origin", project["url"]], log, cwd=dest)
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=dest, capture_output=True, text=True).stdout.strip()
    if head != project["commit"]:
        if run(["git", "fetch", "-q", "--depth", "1", "origin", project["commit"]], log, cwd=dest) != 0:
            run(["git", "fetch", "-q", "--tags", "origin"], log, cwd=dest)
        if run(["git", "checkout", "-q", "--force", project["commit"]], log, cwd=dest) != 0:
            raise RuntimeError(f"cannot check out {project['name']} at {project['commit']}")
    return dest


# ---------------------------------------------------------------------------------------------
# POM helpers


def pom(path):
    root = ET.parse(path).getroot()
    for el in root.iter():
        el.tag = el.tag.split("}")[-1]
    return root


def coordinates(module_dir):
    root = pom(module_dir / "pom.xml")
    group = root.findtext("groupId") or root.findtext("parent/groupId")
    return f"{group}:{root.findtext('artifactId')}", root.findtext("packaging") or "jar"


def modules(project_dir):
    """Every module of the reactor (depth first), as directories."""
    found = []

    def walk(d):
        found.append(d)
        for m in pom(d / "pom.xml").findall("modules/module"):
            walk((d / m.text.strip()).resolve())

    walk(project_dir.resolve())
    return found


# ---------------------------------------------------------------------------------------------
# Class path normalisation: every entry becomes `group:artifact[:classifier]` -> version


def entry_key(path, repo, module_outputs):
    p = os.path.normpath(path)
    if p in module_outputs:
        return module_outputs[p], "(module)"
    if p.startswith(str(repo) + os.sep):
        parts = p[len(str(repo)) + 1 :].split(os.sep)
        if len(parts) >= 4:
            group, artifact, version, file = ".".join(parts[:-3]), parts[-3], parts[-2], parts[-1]
            stem = file.rsplit(".", 1)[0]
            classifier = ""
            base = version[: -len("-SNAPSHOT")] if version.endswith("-SNAPSHOT") else version
            m = re.match(rf"^{re.escape(artifact)}-(?:{re.escape(version)}|{re.escape(base)}-\d{{8}}\.\d{{6}}-\d+)(?:-(.+))?$", stem)
            if m and m.group(1):
                classifier = ":" + m.group(1)
            return f"{group}:{artifact}{classifier}", version
    return p, ""


def normalise(entries, repo, module_outputs, workspace):
    keys, dups = {}, []
    for e in entries:
        key, version = entry_key(e, repo, module_outputs)
        if key in workspace:
            version = "(module)"
        if key in keys:
            dups.append(f"{key} {keys[key]} / {version}")
        else:
            keys[key] = version
    return keys, dups


def diff_class_paths(ours, theirs):
    missing = sorted(f"{k}:{v}" for k, v in theirs.items() if k not in ours)
    extra = sorted(f"{k}:{v}" for k, v in ours.items() if k not in theirs)
    versions = sorted(f"{k} ours {ours[k]} mvn {v}" for k, v in theirs.items() if k in ours and ours[k] != v)
    return missing, extra, versions


# ---------------------------------------------------------------------------------------------
# Checks


class Report:
    def __init__(self):
        self.lines = []
        self.failures = []

    def line(self, text):
        print(text, flush=True)
        self.lines.append(text)

    def fail(self, text):
        self.line("  FAIL " + text)
        self.failures.append(text)


def expected_jdk(level):
    """What the analyzer should choose: the exact version, else the lowest newer one."""
    if level is None:
        return 25
    fitting = [v for v in JDKS if v >= level]
    return min(fitting) if fitting else 25


def mvn_jdk(version):
    """Maven 3.9 itself needs Java 8+."""
    return JDKS[max(version, 8)] if max(version, 8) in JDKS else JAVA_HOME


def query_project(project_dir, home, log, init_options=None):
    """projectClasspath (runtime) for every module. Returns {module_dir: response}."""
    env = env_for(home, {"JA_LOG": "project_model=info"})
    client = LspClient(JA, project_dir, init_options=init_options, env=env, log_path=log)
    try:
        return {m: client.project_classpath(m) for m in modules(project_dir)}
    finally:
        client.shutdown()


def check_classpath(project, report, state):
    name = project["name"]
    plog = LOGS / name
    plog.mkdir(parents=True, exist_ok=True)
    project_dir = checkout(project, plog / "setup.log")
    repo = HOME / ".m2" / "repository"

    # Which JDK does the analyzer choose? Build with it (Maven needs 8+).
    first = query_project(project_dir, HOME, plog / "ja-import.log")
    root_info = first[project_dir.resolve()]
    jdk_version = root_info["jdkVersion"]
    level = max((r["languageLevel"] or 0 for r in first.values()), default=0) or None
    want = expected_jdk(level)
    jdk_ok = jdk_version == want
    report.line(f"{name}: language level {level}, JDK {jdk_version} ({root_info['jdkHome']})"
                + ("" if jdk_ok else f", expected JDK {want}"))
    if not jdk_ok:
        report.fail(f"{name}: chose JDK {jdk_version} for language level {level}, expected {want}")
    for r in first.values():
        if r.get("jdkWarning"):
            report.fail(f"{name}: {r['jdkWarning']}")

    # One reactor build: compile (the launch needs the classes) and write every module's runtime
    # class path; sibling modules resolve to their target/classes within the same build.
    log = plog / "mvn-compile.log"
    log.unlink(missing_ok=True)
    build_env = env_for(HOME, {"JAVA_HOME": str(mvn_jdk(jdk_version))})
    build_env["PATH"] = f"{MAVEN}/bin:{build_env['JAVA_HOME']}/bin:{os.environ['PATH']}"
    out_name = "target/ja-mvn-runtime-cp.txt"
    if mvn(["compile", "dependency:build-classpath", "-DincludeScope=runtime",
            f"-Dmdep.outputFile={out_name}", *MVN_SKIP], log, project_dir, build_env) != 0:
        report.fail(f"{name}: mvn compile failed, see {log}")
        return

    infos = query_project(project_dir, HOME, plog / "ja-classpath.log")
    state[name] = {"dir": project_dir, "jdk": jdk_version, "jdkHome": root_info["jdkHome"], "classpath": infos}
    module_coords = {m: coordinates(m) for m in infos}
    workspace = {c for c, _ in module_coords.values()}
    module_outputs = {}
    for m, (coord, _) in module_coords.items():
        for out in ("target/classes", "target/test-classes"):
            module_outputs[os.path.normpath(m / out)] = coord
    total_dups = total_missing = total_extra = total_versions = 0
    longest = 0
    for m, info in infos.items():
        coord, packaging = module_coords[m]
        if packaging == "pom":
            continue
        own = {os.path.normpath(p) for p in info["outputDirs"]}
        ours_list = [p for p in info["classPaths"] if os.path.normpath(p) not in own]
        longest = max(longest, len(os.pathsep.join(info["classPaths"])))
        mvn_file = m / out_name
        theirs_list = [p for p in mvn_file.read_text().strip().split(os.pathsep) if p] if mvn_file.exists() else []
        ours, dups = normalise(ours_list, repo, module_outputs, workspace)
        theirs, _ = normalise(theirs_list, repo, module_outputs, workspace)
        missing, extra, versions = diff_class_paths(ours, theirs)
        total_dups += len(dups)
        total_missing += len(missing)
        total_extra += len(extra)
        total_versions += len(versions)
        rel = m.relative_to(project_dir.resolve()) if m != project_dir.resolve() else pathlib.Path(".")
        status = "same" if not (dups or missing or extra or versions) else "DIFF"
        report.line(f"  {rel}: {len(ours_list)} entries (mvn {len(theirs_list)}), encoding {info['encoding']}: {status}")
        for label, items in (("duplicate", dups), ("missing", missing), ("extra", extra), ("version", versions)):
            for item in items[:15]:
                report.line(f"    {label}: {item}")
            if len(items) > 15:
                report.line(f"    {label}: ... {len(items) - 15} more")
    report.line(f"  summary: {total_dups} duplicate, {total_missing} missing, {total_extra} extra, "
                f"{total_versions} version differences; longest class path {longest} chars")
    if total_dups or total_missing or total_extra or total_versions:
        report.fail(f"{name}: class path differs from mvn")


def parse_command_debug(text):
    """Rust's `{:?}` of a Command (`cd "dir" && "java" "arg"...`): the program and its arguments."""
    tokens = [json.loads(f'"{s}"') for s in re.findall(r'"((?:[^"\\]|\\.)*)"', text)]
    for i, token in enumerate(tokens):
        if re.search(r"(^|[/\\])java(\.exe)?$", token):
            return tokens[i:]
    return tokens


CONSOLE_LOGBACK = """<configuration>
  <appender name="console" class="ch.qos.logback.core.ConsoleAppender">
    <encoder><pattern>%d{HH:mm:ss.SSS} %-5level %logger{36} - %msg%n</pattern></encoder>
  </appender>
  <root level="INFO"><appender-ref ref="console"/></root>
</configuration>
"""


def launch(project_dir, module_dir, main_class, home, log, settings=None, vm_args=()):
    """Launch through the debug adapter. Returns (outcome, details, command line)."""
    env = env_for(home, {"JA_LOG": "dap=info,project_model=info"})
    dap = DapClient(JA, env=env, cwd=project_dir, log_path=log)
    outcome, details = "FAIL", ""
    try:
        dap.request("initialize", {"clientID": "realproj", "adapterID": "java-analyzer",
                                   "linesStartAt1": True, "columnsStartAt1": True, "locale": "en"})
        args = {"mainClass": main_class, "projectRoot": str(project_dir), "cwd": str(module_dir)}
        if vm_args:
            console_logback = WORK / "logback-console.xml"
            console_logback.write_text(CONSOLE_LOGBACK)
            args["vmArgs"] = [a.format(console_logback=console_logback) for a in vm_args]
        if settings:
            args["mavenSettings"] = str(settings)
        response = dap.request("launch", args, timeout=900)
        if not response.get("success"):
            return "FAIL", f"launch failed: {response.get('message')}\n{dap.text()[-3000:]}", None
        dap.pump(lambda d: "initialized" in d.events, 60)
        dap.request("configurationDone", {}, timeout=60)
        done = lambda d: SPRING_STARTED.search(d.text()) or "terminated" in d.events or "exited" in d.events
        dap.pump(done, LAUNCH_TIMEOUT)
        text = dap.text()
        if SPRING_STARTED.search(text):
            outcome, details = "STARTED", SPRING_STARTED.search(text).group(0)
        elif LINKAGE.search(text):
            outcome, details = "CLASSPATH-BUG", LINKAGE.search(text).group(0)
        elif SERVICES.search(text) and SPRING_LOADING.search(text):
            outcome, details = "SERVICE-DOWN", SERVICES.search(text).group(0)
        else:
            outcome, details = "FAIL", "no Started line" + (" (timeout)" if "terminated" not in dap.events else "")
        try:
            dap.request("disconnect", {"terminateDebuggee": True}, timeout=30)
        except TimeoutError:
            pass
    finally:
        output = dap.text()
        dap.close()
        with open(str(log) + ".out", "w") as f:
            f.write(output)
    command, arg_file = None, None
    for line in open(log, encoding="utf-8", errors="replace"):
        if "启动调试目标:" in line:
            command = parse_command_debug(line.split("启动调试目标:", 1)[1])
        # a long class path goes into an argument file passed as `@file`
        match = re.search(r"类路径在参数文件 (.+?) 里: (.*)$", line.rstrip("\n"))
        if match:
            arg_file = match.groups()
    if command and arg_file and "@" + arg_file[0] in command:
        i = command.index("@" + arg_file[0])
        command[i : i + 1] = ["-cp", arg_file[1]]
    return outcome, details, command


def check_launch(project, report, state):
    name = project["name"]
    if not project["launch"] or name not in state:
        return
    project_dir = state[name]["dir"]
    module_rel, main_class, *vm_args = project["launch"]
    module_dir = (project_dir / module_rel).resolve()
    log = LOGS / name / "dap-launch.log"
    log.unlink(missing_ok=True)
    outcome, details, command = launch(project_dir, module_dir, main_class, HOME, log,
                                       vm_args=vm_args[0] if vm_args else ())
    report.line(f"{name}: launch {main_class} on JDK {state[name]['jdk']}: {outcome} ({details})")
    if outcome in ("FAIL", "CLASSPATH-BUG"):
        report.fail(f"{name}: launch {outcome}: {details}, see {log}.out")
    if command:
        # the debug adapter must use the same JDK and class path as projectClasspath
        info = state[name]["classpath"][module_dir]
        java = command[0]
        if not java.startswith(info["jdkHome"]):
            report.fail(f"{name}: debug adapter runs {java}, projectClasspath says {info['jdkHome']}")
        cp = command[command.index("-cp") + 1].split(os.pathsep) if "-cp" in command else []
        if cp != info["classPaths"]:
            report.fail(f"{name}: debug adapter class path differs from projectClasspath")
        props = [a for a in command[1 : command.index("-cp") if "-cp" in command else None] if a.startswith("-D")]
        report.line(f"  system properties on the java command line: {' '.join(props) or '(none)'}")
    else:
        report.fail(f"{name}: no launch command in {log}")


def check_settings(report, state):
    """A settings.xml with <localRepository> elsewhere; HOME has no ~/.m2/repository at all."""
    alt = WORK / "alt-settings"
    home = alt / "home"
    repo = alt / "repository"
    shutil.rmtree(alt, ignore_errors=True)
    home.mkdir(parents=True)
    prepare_home(home)
    (home / ".m2" / "settings.xml").unlink()
    subprocess.run(["cp", "-al", str(HOME / ".m2" / "repository"), str(repo)], check=True)
    settings = alt / "settings.xml"
    settings.write_text(f"<settings><localRepository>{repo}</localRepository></settings>\n")
    main_repo = str(HOME / ".m2" / "repository")
    for name, data in state.items():
        log = LOGS / name / "ja-alt-settings.log"
        log.unlink(missing_ok=True)
        infos = query_project(data["dir"], home, log, {"maven": {"settings": str(settings)}})
        mvn_calls = [l.strip() for l in open(log, encoding="utf-8", errors="replace") if "mvn（" in l or "无法运行 mvn" in l]
        differs = []
        for m, info in infos.items():
            expected = [p.replace(main_repo, str(repo)) for p in data["classpath"][m]["classPaths"]]
            if info["classPaths"] != expected:
                differs.append(str(m))
        report.line(f"{name}: custom settings.xml: {len(mvn_calls)} mvn calls, "
                    f"{len(differs)} modules with a different class path")
        for call in mvn_calls[:5]:
            report.line(f"    {call}")
        if mvn_calls:
            report.fail(f"{name}: import ran mvn with a complete custom local repository")
        if differs:
            report.fail(f"{name}: class path with custom settings differs in {', '.join(differs[:5])}")
    if "spring-petclinic" in state:
        data = state["spring-petclinic"]
        log = LOGS / "spring-petclinic" / "dap-alt-settings.log"
        log.unlink(missing_ok=True)
        outcome, details, command = launch(data["dir"], data["dir"], PROJECTS[0]["launch"][1], home, log, settings)
        mvn_calls = [l for l in open(log, encoding="utf-8", errors="replace") if "mvn（" in l]
        report.line(f"spring-petclinic: launch with custom settings.xml: {outcome} ({details}), {len(mvn_calls)} mvn calls")
        if outcome != "STARTED" or mvn_calls:
            report.fail(f"spring-petclinic: launch with custom settings.xml: {outcome}, {len(mvn_calls)} mvn calls")
        if command and "-cp" in command:
            cp = command[command.index("-cp") + 1]
            if main_repo + os.sep in cp:
                report.fail("spring-petclinic: debug adapter class path uses ~/.m2/repository despite the custom settings")


GBK_POM = """<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>demo</groupId>
  <artifactId>gbk-demo-{level}</artifactId>
  <version>1.0</version>
  <properties>
    <project.build.sourceEncoding>GBK</project.build.sourceEncoding>
    <maven.compiler.source>{level}</maven.compiler.source>
    <maven.compiler.target>{level}</maven.compiler.target>
  </properties>
</project>
"""
GBK_MAIN = """package demo;

/** 演示：GBK 编码的源文件 */
public class Main {
    public static void main(String[] args) {
        // 调用另一个文件里的中文方法
        String 结果 = Greeter.问候("世界");
        System.out.println(结果);
    }
}
"""
GBK_GREETER = """package demo;

/** 问候工具：注释里有中文 */
public class Greeter {
    /** 前缀 */
    static final String 前缀 = "你好，";

    /* 同一行里的中文注释 */ public static String 问候(String 名字) { return 前缀 + 名字 + "！"; }
}
"""
GBK_EXPECTED = "你好，世界！"


def gbk_locale():
    """Environment for a zh_CN.GBK locale (what a Chinese Windows uses): the installed one, or one
    compiled with localedef into the work directory and found through LOCPATH."""
    installed = subprocess.run(["locale", "-a"], capture_output=True, text=True).stdout.split()
    for name in installed:
        if name.lower() == "zh_cn.gbk":
            return {"LANG": name, "LC_ALL": name}, f"{name} (installed)"
    locales = WORK / "locale"
    if not (locales / "zh_CN.GBK").is_dir():
        locales.mkdir(exist_ok=True)
        try:
            subprocess.run(["localedef", "-i", "zh_CN", "-f", "GBK", str(locales / "zh_CN.GBK")],
                           capture_output=True, timeout=120)
        except (OSError, subprocess.TimeoutExpired):
            pass
    if (locales / "zh_CN.GBK").is_dir():
        return {"LANG": "zh_CN.GBK", "LC_ALL": "zh_CN.GBK", "LOCPATH": str(locales)}, "zh_CN.GBK (localedef, LOCPATH)"
    return {}, f"zh_CN.GBK unavailable, using {os.environ.get('LANG', 'C')}"


def check_gbk(report, level):
    name = f"gbk-demo-{level}"
    project_dir = WORK / name
    shutil.rmtree(project_dir, ignore_errors=True)
    src = project_dir / "src/main/java/demo"
    src.mkdir(parents=True)
    (project_dir / "pom.xml").write_text(GBK_POM.format(level=level))
    (src / "Main.java").write_bytes(GBK_MAIN.encode("gbk"))
    (src / "Greeter.java").write_bytes(GBK_GREETER.encode("gbk"))
    plog = LOGS / name
    shutil.rmtree(plog, ignore_errors=True)
    plog.mkdir(parents=True)

    lang, where = gbk_locale()
    report.line(f"{name}: locale {where}")

    env = env_for(HOME, {"JA_LOG": "project_model=info", **lang})
    client = LspClient(JA, project_dir, env=env, log_path=plog / "ja-lsp.log")
    try:
        main = src / "Main.java"
        greeter = src / "Greeter.java"
        info = client.project_classpath(project_dir)
        report.line(f"  encoding {info['encoding']}, JDK {info['jdkVersion']}")
        if (info["encoding"] or "").upper() != "GBK":
            report.fail(f"{name}: projectClasspath encoding is {info['encoding']}")
        client.did_open(main, GBK_MAIN)
        # goto definition of 问候 into the unopened, GBK-encoded Greeter.java
        line_no = GBK_MAIN.splitlines().index('        String 结果 = Greeter.问候("世界");')
        col = GBK_MAIN.splitlines()[line_no].index("问候")
        result, _ = client.request("textDocument/definition",
                                   {"textDocument": {"uri": file_uri(main)}, "position": {"line": line_no, "character": col}})
        locations = result if isinstance(result, list) else [result] if result else []
        want_line = next(i for i, l in enumerate(GBK_GREETER.splitlines()) if "问候(String" in l)
        want_col = GBK_GREETER.splitlines()[want_line].index("问候")
        got = [(l.get("uri") or l.get("targetUri"), (l.get("range") or l.get("targetSelectionRange"))["start"]) for l in locations]
        ok = any(u == file_uri(greeter) and s == {"line": want_line, "character": want_col} for u, s in got)
        report.line(f"  goto definition 问候: {got} (want {want_line}:{want_col}) {'ok' if ok else ''}")
        if not ok:
            report.fail(f"{name}: goto definition into GBK file landed at {got}, want line {want_line} col {want_col}")
        errors = []
        for path, text in ((main, GBK_MAIN), (greeter, GBK_GREETER)):
            if path == greeter:
                client.did_open(greeter, GBK_GREETER)
            result, _ = client.request("textDocument/diagnostic", {"textDocument": {"uri": file_uri(path)}})
            errors += [f"{path.name}:{d['range']['start']['line'] + 1}: {d['message']}"
                       for d in (result or {}).get("items", []) if d.get("severity") == 1]
        report.line(f"  error diagnostics: {len(errors)}")
        for e in errors[:10]:
            report.line(f"    {e}")
        if errors:
            report.fail(f"{name}: {len(errors)} error diagnostics in GBK sources")
    finally:
        client.shutdown()

    build_env = env_for(HOME, {"JAVA_HOME": str(mvn_jdk(info["jdkVersion"]))})
    build_env["PATH"] = f"{MAVEN}/bin:{build_env['JAVA_HOME']}/bin:{os.environ['PATH']}"
    if mvn(["compile"], plog / "mvn.log", project_dir, build_env) != 0:
        report.fail(f"{name}: mvn compile failed, see {plog / 'mvn.log'}")
        return
    log = plog / "dap-launch.log"
    dap_env_home = HOME
    env = env_for(dap_env_home, {"JA_LOG": "dap=info", **lang})
    dap = DapClient(JA, env=env, cwd=project_dir, log_path=log)
    try:
        dap.request("initialize", {"clientID": "realproj", "adapterID": "java-analyzer", "locale": "zh-cn"})
        response = dap.request("launch", {"mainClass": "demo.Main", "projectRoot": str(project_dir)})
        if response.get("success"):
            dap.pump(lambda d: "initialized" in d.events, 60)
            dap.request("configurationDone", {}, timeout=60)
            dap.pump(lambda d: "terminated" in d.events, 60)
        stdout = "".join(t for c, t in dap.output if c == "stdout")
    finally:
        dap.close()
    command = [l.split("启动调试目标:", 1)[1] for l in open(log, encoding="utf-8", errors="replace") if "启动调试目标:" in l]
    vm_args = [a for a in parse_command_debug(command[0]) if a.startswith("-D")] if command else []
    ok = stdout.strip() == GBK_EXPECTED
    report.line(f"  launch output {stdout.strip()!r} with {' '.join(vm_args)}: {'ok' if ok else 'WRONG'}")
    if not ok:
        report.fail(f"{name}: launched app printed {stdout.strip()!r}, want {GBK_EXPECTED!r}")


def check_stats(project, report):
    name = project["name"]
    project_dir = checkout(project, LOGS / name / "setup.log")
    log = LOGS / name / "analysis-stats.log"
    log.unlink(missing_ok=True)
    start = time.monotonic()
    code = run([JA, "analysis-stats", str(project_dir)], log, env=env_for(HOME), timeout=3600)
    text = log.read_text(errors="replace")
    panicked = "panicked" in text
    errors = re.findall(r"(?:语义错误|Semantic errors)[:：] *\d+", text)
    report.line(f"{name}: analysis-stats exit {code} in {time.monotonic() - start:.0f}s"
                f"{', PANICKED' if panicked else ''}; {errors[0] if errors else ''}")
    if code != 0 or panicked:
        report.fail(f"{name}: analysis-stats exit {code}{' (panic)' if panicked else ''}, see {log}")


def main():
    checks = sys.argv[1:] or ["classpath", "launch", "settings", "gbk", "stats"]
    unknown = set(checks) - {"classpath", "launch", "settings", "gbk", "stats"}
    if unknown:
        sys.exit(f"unknown checks: {', '.join(sorted(unknown))}")
    if ("launch" in checks or "settings" in checks) and "classpath" not in checks:
        checks.insert(0, "classpath")  # they need its build and results
    only = {n for n in os.environ.get("JA_REALPROJ_ONLY", "").split(",") if n}
    projects = [p for p in PROJECTS if not only or p["name"] in only]
    LOGS.mkdir(parents=True, exist_ok=True)
    prepare_home(HOME)
    report = Report()
    report.line(f"java-analyzer {subprocess.run([JA, '--version'], capture_output=True, text=True).stdout.strip()}, "
                f"JAVA_HOME JDK 25, discoverable JDKs {sorted(v for v in JDKS if v != 25)}")
    state = {}
    for project in projects:
        for check in ("classpath", "launch"):
            if check in checks:
                report.line(f"== {check}: {project['name']}")
                try:
                    (check_classpath if check == "classpath" else check_launch)(project, report, state)
                except Exception as e:  # keep going with the other projects
                    report.fail(f"{project['name']}: {check} crashed: {e!r}")
    if "settings" in checks:
        report.line("== settings")
        try:
            check_settings(report, state)
        except Exception as e:
            report.fail(f"settings crashed: {e!r}")
    if "gbk" in checks:
        for level in (8, 21):
            report.line("== gbk")
            try:
                check_gbk(report, level)
            except Exception as e:
                report.fail(f"gbk-demo-{level} crashed: {e!r}")
    if "stats" in checks:
        report.line("== stats")
        for project in projects:
            check_stats(project, report)
    report.line(f"== {len(report.failures)} failures")
    for f in report.failures:
        report.line(f"  {f}")
    (WORK / "report.txt").write_text("\n".join(report.lines) + "\n")
    sys.exit(1 if report.failures else 0)


if __name__ == "__main__":
    main()
