//- /.editorconfig
[*.java]
ij_java_names_count_to_use_static_import_on_demand = 3
//- /app/Collapse.java
package app;

import static java.lang.Math.abs;
import static java.lang.Math.max;
import static java.lang.Math.min;

class /*@collapse*/Collapse {
    int f(int a, int b) { return abs(max(a, b) - min(a, b)); }
}
//- /app/Expand.java
package app;

import static java.util.concurrent.TimeUnit.*;

import java.util.concurrent.TimeUnit;

class /*@expand*/Expand {
    TimeUnit unit = SECONDS;
}
//- /app/Conflicts.java
package app;

import static java.lang.Long.*;
import static java.lang.Integer.MAX_VALUE;
import static java.lang.Integer.parseInt;
import static java.lang.Integer.toHexString;

class /*@conflicts*/Conflicts {
    long a = MAX_VALUE + parseInt("1") + parseLong("2");
    String b = toHexString(1) + toBinaryString(2L) + numberOfLeadingZeros(3L);
}
//? assist organize_imports collapse ; has "import static java.lang.Math.*;" ; lacks "import static java.lang.Math.max;"
//? assist organize_imports expand ; has "import static java.util.concurrent.TimeUnit.SECONDS;" ; lacks "import static java.util.concurrent.TimeUnit.*;"
//? assist organize_imports conflicts ; has "import static java.lang.Long.*;" ; has "import static java.lang.Integer.parseInt;" ; lacks "import static java.lang.Integer.*;"
