//! Introspection XML for the objects we serve.

pub const MANAGER: &str = r#"
 <interface name="org.freedesktop.systemd1.Manager">
  <method name="GetUnit"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetUnitByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="LoadUnit"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="StartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="StopUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ReloadUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="RestartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="TryRestartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ReloadOrRestartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ReloadOrTryRestartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="KillUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="i" direction="in"/></method>
  <method name="ResetFailedUnit"><arg type="s" direction="in"/></method>
  <method name="ResetFailed"/>
  <method name="GetUnitProcesses"><arg type="s" direction="in"/><arg type="a(sus)" direction="out"/></method>
  <method name="StartTransientUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="a(sv)" direction="in"/><arg type="a(sa(sv))" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetJob"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="CancelJob"><arg type="u" direction="in"/></method>
  <method name="ClearJobs"/>
  <method name="ListUnits"><arg type="a(ssssssouso)" direction="out"/></method>
  <method name="ListUnitsFiltered"><arg type="as" direction="in"/><arg type="a(ssssssouso)" direction="out"/></method>
  <method name="ListUnitsByPatterns"><arg type="as" direction="in"/><arg type="as" direction="in"/><arg type="a(ssssssouso)" direction="out"/></method>
  <method name="ListUnitsByNames"><arg type="as" direction="in"/><arg type="a(ssssssouso)" direction="out"/></method>
  <method name="ListJobs"><arg type="a(usssoo)" direction="out"/></method>
  <method name="Subscribe"/>
  <method name="Unsubscribe"/>
  <method name="Reload"/>
  <method name="Reexecute"/>
  <method name="PowerOff"/>
  <method name="Reboot"/>
  <method name="Halt"/>
  <method name="SetEnvironment"><arg type="as" direction="in"/></method>
  <method name="UnsetEnvironment"><arg type="as" direction="in"/></method>
  <method name="UnsetAndSetEnvironment"><arg type="as" direction="in"/><arg type="as" direction="in"/></method>
  <method name="ListUnitFiles"><arg type="a(ss)" direction="out"/></method>
  <method name="ListUnitFilesByPatterns"><arg type="as" direction="in"/><arg type="as" direction="in"/><arg type="a(ss)" direction="out"/></method>
  <method name="GetUnitFileState"><arg type="s" direction="in"/><arg type="s" direction="out"/></method>
  <method name="EnableUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="EnableUnitFilesWithFlags"><arg type="as" direction="in"/><arg type="t" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="DisableUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <method name="DisableUnitFilesWithFlags"><arg type="as" direction="in"/><arg type="t" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <method name="DisableUnitFilesWithFlagsAndInstallInfo"><arg type="as" direction="in"/><arg type="t" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="ReenableUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="MaskUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <method name="UnmaskUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <method name="PresetUnitFiles"><arg type="as" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="PresetUnitFilesWithMode"><arg type="as" direction="in"/><arg type="s" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="out"/><arg type="a(sss)" direction="out"/></method>
  <method name="PresetAllUnitFiles"><arg type="s" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <method name="GetDefaultTarget"><arg type="s" direction="out"/></method>
  <method name="SetDefaultTarget"><arg type="s" direction="in"/><arg type="b" direction="in"/><arg type="a(sss)" direction="out"/></method>
  <signal name="UnitNew"><arg type="s"/><arg type="o"/></signal>
  <signal name="UnitRemoved"><arg type="s"/><arg type="o"/></signal>
  <signal name="JobNew"><arg type="u"/><arg type="o"/><arg type="s"/></signal>
  <signal name="JobRemoved"><arg type="u"/><arg type="o"/><arg type="s"/><arg type="s"/></signal>
  <signal name="StartupFinished"><arg type="t"/><arg type="t"/><arg type="t"/><arg type="t"/><arg type="t"/><arg type="t"/></signal>
  <signal name="UnitFilesChanged"/>
  <signal name="Reloading"><arg type="b"/></signal>
 </interface>
"#;

pub const UNIT: &str = r#"
 <interface name="org.freedesktop.systemd1.Unit">
  <method name="Start"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="Stop"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="Reload"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="Restart"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="TryRestart"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ReloadOrRestart"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ReloadOrTryRestart"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="Kill"><arg type="s" direction="in"/><arg type="i" direction="in"/></method>
  <method name="ResetFailed"/>
 </interface>
"#;

pub const STANDARD: &str = r#"
 <interface name="org.freedesktop.DBus.Peer">
  <method name="Ping"/>
  <method name="GetMachineId"><arg type="s" direction="out"/></method>
 </interface>
 <interface name="org.freedesktop.DBus.Introspectable">
  <method name="Introspect"><arg type="s" direction="out"/></method>
 </interface>
 <interface name="org.freedesktop.DBus.Properties">
  <method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <method name="GetAll"><arg type="s" direction="in"/><arg type="a{sv}" direction="out"/></method>
  <method name="Set"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/></method>
  <signal name="PropertiesChanged"><arg type="s"/><arg type="a{sv}"/><arg type="as"/></signal>
 </interface>
"#;

pub fn node(body: &str, children: &[String]) -> String {
    let mut s = String::from(
        "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n\"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n",
    );
    s.push_str(STANDARD);
    s.push_str(body);
    for c in children {
        s.push_str(&format!(" <node name=\"{c}\"/>\n"));
    }
    s.push_str("</node>\n");
    s
}
