/// Kuzz's public contact and policy addresses, in one place.
library;

/// Where "Contact us" in Settings writes to.
// TODO(akak): replace with the support address before store submission.
const kuzzContactEmail = 'jijeegee@gmail.com';

/// The Terms of Service the sign-in screen links to.
// TODO(akak): replace with the published terms page before store submission.
const kuzzTermsUrl = 'https://example.com/kuzz/terms';

/// A `mailto:` link to [kuzzContactEmail].
final kuzzContactUri = Uri(scheme: 'mailto', path: kuzzContactEmail);
