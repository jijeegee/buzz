import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// Bytes the notification extension may download in one run, shared by
/// every bounded fetch of that run.
public final class ResponseByteBudget: @unchecked Sendable {
  private let lock = NSLock()
  private var remaining: Int

  public init(bytes: Int) { remaining = bytes }

  public var available: Int {
    lock.lock()
    defer { lock.unlock() }
    return remaining
  }

  /// Takes `count` bytes, or none and false when fewer remain.
  func take(_ count: Int) -> Bool {
    lock.lock()
    defer { lock.unlock() }
    guard count <= remaining else { return false }
    remaining -= count
    return true
  }
}

/// One HTTP request with a byte cap and a wall-clock deadline, both of
/// which cancel it (a URLRequest timeout only bounds idle time).
///
/// Runs on its own URLSession built from the caller's configuration, with
/// this object as the delegate, and invalidates it when done.
final class BoundedDataTask: NSObject, URLSessionDataDelegate, @unchecked Sendable {
  enum Outcome {
    case response(HTTPURLResponse, Data)
    /// Over the byte cap or past the deadline.
    case cancelled
    case failed
  }

  private let budget: ResponseByteBudget
  private let maxBytes: Int
  private let completion: (Outcome) -> Void
  private let lock = NSLock()
  private var received = Data()
  private var response: HTTPURLResponse?
  private var finished = false
  private var session: URLSession?
  private var task: URLSessionDataTask?

  private init(
    budget: ResponseByteBudget, maxBytes: Int, completion: @escaping (Outcome) -> Void
  ) {
    self.budget = budget
    self.maxBytes = maxBytes
    self.completion = completion
  }

  /// Starts `request`. `completion` runs exactly once, off the caller's
  /// thread. A deadline already past cancels at once.
  static func start(
    _ request: URLRequest,
    configuration: URLSessionConfiguration,
    budget: ResponseByteBudget,
    maxBytes: Int,
    deadline: Date,
    now: Date = Date(),
    completion: @escaping (Outcome) -> Void
  ) {
    let bounded = BoundedDataTask(budget: budget, maxBytes: maxBytes, completion: completion)
    let remaining = deadline.timeIntervalSince(now)
    guard remaining > 0 else {
      DispatchQueue.global(qos: .userInitiated).async { completion(.cancelled) }
      return
    }
    var request = request
    request.timeoutInterval = min(request.timeoutInterval, remaining)
    let session = URLSession(configuration: configuration, delegate: bounded, delegateQueue: nil)
    let task = session.dataTask(with: request)
    bounded.lock.lock()
    bounded.session = session
    bounded.task = task
    bounded.lock.unlock()
    DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + remaining) {
      bounded.finish(.cancelled)
    }
    task.resume()
  }

  func urlSession(
    _ session: URLSession,
    dataTask: URLSessionDataTask,
    didReceive response: URLResponse,
    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
  ) {
    let http = response as? HTTPURLResponse
    lock.lock()
    self.response = http
    lock.unlock()
    let expected = response.expectedContentLength
    if expected > 0, expected > Int64(min(maxBytes, budget.available)) {
      completionHandler(.cancel)
      finish(.cancelled)
      return
    }
    completionHandler(.allow)
  }

  func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
    lock.lock()
    let overCap = received.count + data.count > maxBytes
    lock.unlock()
    guard !overCap, budget.take(data.count) else {
      finish(.cancelled)
      return
    }
    lock.lock()
    received.append(data)
    lock.unlock()
  }

  func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
    lock.lock()
    let response = self.response
    let data = received
    lock.unlock()
    if error == nil, let response {
      finish(.response(response, data))
    } else {
      finish(.failed)
    }
  }

  private func finish(_ outcome: Outcome) {
    lock.lock()
    guard !finished else {
      lock.unlock()
      return
    }
    finished = true
    let session = self.session
    let task = self.task
    self.session = nil
    self.task = nil
    lock.unlock()
    task?.cancel()
    session?.invalidateAndCancel()
    completion(outcome)
  }
}
