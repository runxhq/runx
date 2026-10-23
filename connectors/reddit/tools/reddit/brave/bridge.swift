// Fixed macOS transport. PID + window + tab identify one running Brave instance.
// The public connector never exposes this internal JavaScript transport.
import AppKit
import ScriptingBridge

func code(_ text: String) -> AEKeyword { text.utf8.reduce(0) { ($0 << 8) | UInt32($1) } }
func property(_ object: SBObject, _ name: String) -> Any? { object.property(withCode: code(name)).get() }
func identifier(_ object: SBObject) -> String { String(describing: property(object, "ID  ") ?? "") }
func specifier(_ kind: String, _ id: String, _ container: NSAppleEventDescriptor) -> NSAppleEventDescriptor {
    let record = NSAppleEventDescriptor.record()
    record.setDescriptor(NSAppleEventDescriptor(typeCode: code(kind)), forKeyword: code("want"))
    record.setDescriptor(NSAppleEventDescriptor(enumCode: code("ID  ")), forKeyword: code("form"))
    record.setDescriptor(NSAppleEventDescriptor(string: id), forKeyword: code("seld"))
    record.setDescriptor(container, forKeyword: code("from"))
    return record.coerce(toDescriptorType: code("obj "))!
}
func emit(_ value: Any) throws { print(String(decoding: try JSONSerialization.data(withJSONObject: value, options:[.fragmentsAllowed]), as: UTF8.self)) }
struct Stop: Error { let reason: String }
func execute(_ request: [String: Any]) throws -> Any {
    let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.brave.Browser")
    if request["operation"] as? String == "running" {return ["running": !running.isEmpty]}
    if request["operation"] as? String == "tabs" {
        var rows: [[String: String]] = []
        for process in running {
            guard let app = SBApplication(processIdentifier: process.processIdentifier) else {continue}
            for case let window as SBObject in app.elementArray(withCode: code("cwin")) {
                guard property(window, "mode") as? String == "normal" else {continue}
                for case let tab as SBObject in window.elementArray(withCode: code("CrTb")) {
                    let url = property(tab, "URL ") as? String ?? ""
                    if ["www.reddit.com", "old.reddit.com", "reddit.com"].contains(URL(string:url)?.host ?? "") {
                        rows.append(["browser_pid": String(process.processIdentifier), "window_id": identifier(window), "tab_id": identifier(tab), "url": url])
                    }
                }
            }
        }
        return ["tabs": Array(rows.prefix(30))]
    }
    guard let binding = request["binding"] as? [String:String],
          let pid = Int32(binding["browser_pid"] ?? ""),
          running.contains(where: {$0.processIdentifier == pid}),
          let app = SBApplication(processIdentifier: pid) else {throw Stop(reason:"missing_browser_instance")}
    let windows = app.elementArray(withCode: code("cwin"))
    guard let window = windows.compactMap({$0 as? SBObject}).first(where: {identifier($0) == binding["window_id"]}),
          property(window,"mode") as? String == "normal",
          let tab = window.elementArray(withCode: code("CrTb")).compactMap({$0 as? SBObject}).first(where: {identifier($0) == binding["tab_id"]}) else {throw Stop(reason:"missing_bound_tab")}
    guard let front = NSWorkspace.shared.frontmostApplication else {throw Stop(reason:"foreground_state_unknown")}
    let frontWindow = windows.firstObject as? SBObject
    let active = frontWindow.flatMap {property($0,"acTa") as? SBObject}
    let foreground = front.processIdentifier == pid && frontWindow.map(identifier) == binding["window_id"] && active.map(identifier) == binding["tab_id"]
    if request["operation"] as? String == "foreground" {return ["foreground": foreground]}
    if front.processIdentifier == pid && active == nil {throw Stop(reason:"foreground_state_unknown")}
    if foreground {throw Stop(reason:"bound_tab_is_foreground")}
    if request["operation"] as? String == "navigate", let url = request["url"] as? String {
        guard ["www.reddit.com", "old.reddit.com"].contains(URL(string:url)?.host ?? ""), URL(string:url)?.scheme == "https" else {throw Stop(reason:"non_reddit_url")}
        tab.property(withCode:code("URL ")).setTo(url)
        return ["navigated": true]
    }
    guard request["operation"] as? String == "execute", let script = request["javascript"] as? String else {throw Stop(reason:"invalid_bridge_command")}
    let currentURL = property(tab,"URL ") as? String ?? ""
    guard ["www.reddit.com", "old.reddit.com"].contains(URL(string:currentURL)?.host ?? "") else {throw Stop(reason:"non_reddit_url")}
    let event = NSAppleEventDescriptor(eventClass:code("CrSu"), eventID:code("ExJa"), targetDescriptor:NSAppleEventDescriptor(processIdentifier:pid), returnID:-1, transactionID:0)
    let windowSpecifier = specifier("cwin", binding["window_id"]!, NSAppleEventDescriptor.null())
    event.setParam(specifier("CrTb", binding["tab_id"]!, windowSpecifier), forKeyword:code("----"))
    event.setParam(NSAppleEventDescriptor(string:script), forKeyword:code("JvSc"))
    let reply = try event.sendEvent(options:[.waitForReply, .neverInteract], timeout:15)
    guard let value = reply.paramDescriptor(forKeyword:code("----"))?.stringValue,
          let bytes = value.data(using:.utf8) else {throw Stop(reason:"invalid_browser_result")}
    return try JSONSerialization.jsonObject(with:bytes, options:[.fragmentsAllowed])
}
do {
    guard CommandLine.arguments.count == 2,
          let data = CommandLine.arguments[1].data(using:.utf8),
          let request = try JSONSerialization.jsonObject(with:data) as? [String:Any] else {throw Stop(reason:"invalid_bridge_command")}
    try emit(execute(request))
} catch let error as Stop {try emit(["error":error.reason])}
catch {try emit(["error":"apple_event_failed_\((error as NSError).code)"])}
