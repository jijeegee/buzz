/// Kuzz's public contact and policy addresses, in one place.
library;

/// Where "Contact us" in Settings writes to.
const kuzzContactEmail = 'support@kuzz.app';

/// The Terms of Service the sign-in screen links to.
const kuzzTermsUrl = 'https://kuzz.app/terms';

/// The Privacy Policy the sign-in screen links to.
const kuzzPrivacyUrl = 'https://kuzz.app/privacy';

/// A `mailto:` link to [kuzzContactEmail].
final kuzzContactUri = Uri(scheme: 'mailto', path: kuzzContactEmail);
