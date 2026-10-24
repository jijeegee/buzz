import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../theme/theme.dart';

/// Google's four-colour "G", drawn from its brand mark.
const _googleMark =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">'
    '<path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85'
    'C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19'
    'C12.43 13.72 17.74 9.5 24 9.5z"/>'
    '<path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02'
    'h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 '
    '7.09-17.65z"/>'
    '<path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59'
    's.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 '
    '2.56 10.78l7.97-6.19z"/>'
    '<path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6'
    'c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19'
    'C6.51 42.62 14.62 48 24 48z"/>'
    '</svg>';

/// The small mark of the method an account signs in with: Google's "G", or
/// the Apple logo for `apple`.
class SignInMethodIcon extends StatelessWidget {
  const SignInMethodIcon({super.key, required this.method, this.size = 18});

  /// `google` or `apple` (see `Community.signInMethod`).
  final String method;
  final double size;

  @override
  Widget build(BuildContext context) {
    if (method == 'apple') {
      return Icon(
        Icons.apple,
        key: const Key('sign-in-method-apple'),
        size: size,
        color: context.colors.onSurface,
      );
    }
    return SvgPicture.string(
      _googleMark,
      key: const Key('sign-in-method-google'),
      width: size,
      height: size,
    );
  }
}
