// Smoke test for the Swift bindings: two documents sync through updates and the observer.
// Run with `yuniffi/test-swift.sh`.

import Foundation

func check(_ condition: Bool, _ message: String) {
    if !condition {
        print("FAIL: \(message)")
        exit(1)
    }
}

final class Collect: UpdateObserver, @unchecked Sendable {
    private let lock = NSLock()
    private var collected: [Data] = []
    var updates: [Data] { lock.withLock { collected } }
    func onUpdate(update: Data) { lock.withLock { collected.append(update) } }
}

let a = try YDoc.withClientId(clientId: 1)
let b = try YDoc.withClientId(clientId: 2)
check(a.clientId() == 1, "client ID")

let seen = Collect()
a.observeUpdates(key: "sync", observer: seen)
a.text(name: "t").push(chunk: "hello")
try a.map(name: "m").set(key: "k", value: .string(value: "v"))
try a.array(name: "l").push(value: .int(value: 42))
check(seen.updates.count == 3, "one update per commit, got \(seen.updates.count)")

for update in seen.updates {
    try b.applyUpdate(update: update)
}
check(b.text(name: "t").getString() == "hello", "text synced")
check(b.map(name: "m").get(key: "k") == .string(value: "v"), "map synced")
check(b.array(name: "l").toList() == [.int(value: 42)], "array synced")

let diff = try b.encodeStateAsUpdate(stateVector: a.stateVector())
let other = YDoc()
try other.applyUpdate(update: try mergeUpdates(updates: seen.updates + [diff]))
check(other.text(name: "t").getString() == "hello", "merged updates apply")

do {
    try a.text(name: "t").insert(index: 99, chunk: "x")
    check(false, "insert past the end must throw")
} catch YError.OutOfRange {
}
do {
    try a.applyUpdate(update: Data([0xff, 0xff]))
    check(false, "a bad update must throw")
} catch YError.InvalidEncoding {
}

print("swift smoke test: ok")
