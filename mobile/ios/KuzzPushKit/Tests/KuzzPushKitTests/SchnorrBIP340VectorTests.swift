import Foundation
import P256K
import XCTest

/// BIP-340 ground truth for the Schnorr verification P256K performs on our
/// behalf, so a `swift-secp256k1` version change cannot silently alter which
/// Nostr signatures the push extension accepts.
///
/// Vectors 0-4 (valid) and 5-14 (invalid) are from the BIP-340 reference
/// `test-vectors.csv`.
final class SchnorrBIP340VectorTests: XCTestCase {
  private struct Vector {
    let index: Int
    let publicKey: String
    let message: String
    let signature: String
    let isValid: Bool
  }

  // swiftlint:disable line_length
  private static let vectors: [Vector] = [
    Vector(
      index: 0,
      publicKey: "F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9",
      message: "0000000000000000000000000000000000000000000000000000000000000000",
      signature:
        "E907831F80848D1069A5371B402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0",
      isValid: true
    ),
    Vector(
      index: 1,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "6896BD60EEAE296DB48A229FF71DFE071BDE413E6D43F917DC8DCF8C78DE33418906D11AC976ABCCB20B091292BFF4EA897EFCB639EA871CFA95F6DE339E4B0A",
      isValid: true
    ),
    Vector(
      index: 2,
      publicKey: "DD308AFEC5777E13121FA72B9CC1B7CC0139715309B086C960E18FD969774EB8",
      message: "7E2D58D8B3BCDF1ABADEC7829054F90DDA9805AAB56C77333024B9D0A508B75C",
      signature:
        "5831AAEED7B44BB74E5EAB94BA9D4294C49BCF2A60728D8B4C200F50DD313C1BAB745879A5AD954A72C45A91C3A51D3C7ADEA98D82F8481E0E1E03674A6F3FB7",
      isValid: true
    ),
    Vector(
      index: 3,
      publicKey: "25D1DFF95105F5253C4022F628A996AD3A0D95FBF21D468A1B33F8C160D8F517",
      message: "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
      signature:
        "7EB0509757E246F19449885651611CB965ECC1A187DD51B64FDA1EDC9637D5EC97582B9CB13DB3933705B32BA982AF5AF25FD78881EBB32771FC5922EFC66EA3",
      isValid: true
    ),
    Vector(
      index: 4,
      publicKey: "D69C3509BB99E412E68B0FE8544E72837DFA30746D8BE2AA65975F29D22DC7B9",
      message: "4DF3C3F68FCC83B27E9D42C90431A72499F17875C81A599B566C9889B9696703",
      signature:
        "00000000000000000000003B78CE563F89A0ED9414F5AA28AD0D96D6795F9C6376AFB1548AF603B3EB45C9F8207DEE1060CB71C04E80F593060B07D28308D7F4",
      isValid: true
    ),
    Vector(
      index: 5,
      publicKey: "EEFDEA4CDB677750A420FEE807EACF21EB9898AE79B9768766E4FAA04A2D4A34",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
      isValid: false
    ),
    Vector(
      index: 6,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "FFF97BD5755EEEA420453A14355235D382F6472F8568A18B2F057A14602975563CC27944640AC607CD107AE10923D9EF7A73C643E166BE5EBEAFA34B1AC553E2",
      isValid: false
    ),
    Vector(
      index: 7,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "1FA62E331EDBC21C394792D2AB1100A7B432B013DF3F6FF4F99FCB33E0E1515F28890B3EDB6E7189B630448B515CE4F8622A954CFE545735AAEA5134FCCDB2BD",
      isValid: false
    ),
    Vector(
      index: 8,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769961764B3AA9B2FFCB6EF947B6887A226E8D7C93E00C5ED0C1834FF0D0C2E6DA6",
      isValid: false
    ),
    Vector(
      index: 9,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "0000000000000000000000000000000000000000000000000000000000000000123DDA8328AF9C23A94C1FEECFD123BA4FB73476F0D594DCB65C6425BD186051",
      isValid: false
    ),
    Vector(
      index: 10,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "00000000000000000000000000000000000000000000000000000000000000017615FBAF5AE28864013C099742DEADB4DBA87F11AC6754F93780D5A1837CF197",
      isValid: false
    ),
    Vector(
      index: 11,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "4A298DACAE57395A15D0795DDBFD1DCB564DA82B0F269BC70A74F8220429BA1D69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
      isValid: false
    ),
    Vector(
      index: 12,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
      isValid: false
    ),
    Vector(
      index: 13,
      publicKey: "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
      isValid: false
    ),
    Vector(
      index: 14,
      publicKey: "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC30",
      message: "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
      signature:
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
      isValid: false
    ),
  ]
  // swiftlint:enable line_length

  func testSchnorrVerificationMatchesBIP340Vectors() throws {
    for vector in Self.vectors {
      let publicKey = try XCTUnwrap(
        Self.bytes(vector.publicKey), "vector \(vector.index) public key")
      let signatureBytes = try XCTUnwrap(
        Self.bytes(vector.signature), "vector \(vector.index) signature")
      var message = try XCTUnwrap(Self.bytes(vector.message), "vector \(vector.index) message")

      guard
        let signature = try? P256K.Schnorr.SchnorrSignature(dataRepresentation: signatureBytes)
      else {
        XCTAssertFalse(vector.isValid, "vector \(vector.index) signature failed to parse")
        continue
      }
      let key = P256K.Schnorr.XonlyKey(dataRepresentation: publicKey)

      XCTAssertEqual(
        key.isValid(signature, for: &message),
        vector.isValid,
        "vector \(vector.index)"
      )
    }
  }

  /// A signature this build produces must verify under the same public key, so
  /// signing and verification cannot drift apart together.
  func testSignatureRoundTripsThroughVerification() throws {
    let privateKey = try P256K.Schnorr.PrivateKey()
    var message = Array("kuzz push enrollment".utf8)
    var auxiliary = Array(repeating: UInt8(0), count: 32)
    let signature = try privateKey.signature(message: &message, auxiliaryRand: &auxiliary)

    XCTAssertTrue(privateKey.xonly.isValid(signature, for: &message))

    var tampered = message
    tampered[0] ^= 0x01
    XCTAssertFalse(privateKey.xonly.isValid(signature, for: &tampered))
  }

  private static func bytes(_ hex: String) -> [UInt8]? {
    guard hex.count.isMultiple(of: 2) else { return nil }
    var result: [UInt8] = []
    result.reserveCapacity(hex.count / 2)
    var index = hex.startIndex
    while index < hex.endIndex {
      let next = hex.index(index, offsetBy: 2)
      guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
      result.append(byte)
      index = next
    }
    return result
  }
}
