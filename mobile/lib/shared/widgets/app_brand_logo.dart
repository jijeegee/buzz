import 'package:flutter/material.dart';

import '../theme/theme.dart';
import 'kuzz_mark.dart';

/// Product name shown where the server (community) name used to be.
const appBrandName = 'Kuzz';

/// The app logo in a round badge. Used instead of a community icon when the
/// app runs against its built-in server.
class AppBrandLogo extends StatelessWidget {
  const AppBrandLogo({super.key, this.size = 40});

  final double size;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: appBrandName,
      image: true,
      child: Container(
        width: size,
        height: size,
        decoration: BoxDecoration(
          color: context.colors.primaryContainer,
          shape: BoxShape.circle,
        ),
        alignment: Alignment.center,
        child: KuzzMark(height: size * 0.5, color: context.colors.primary),
      ),
    );
  }
}
