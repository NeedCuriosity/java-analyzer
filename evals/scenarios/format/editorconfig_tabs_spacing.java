//- /.editorconfig
[*.java]
indent_style = tab
tab_width = 4
ij_java_space_before_method_call_parentheses = true
ij_java_space_within_parentheses = true
ij_java_space_around_additive_operators = false
ij_java_space_after_comma = false
ij_java_brace_style = next_line
//- /Tabs.java
class Tabs {
    int f(int a, int b) {
        if (a > b) {
            return g(a, (a + b) * 2);
        } else {
            return 0;
        }
    }
    int g(int x, int y) {
        return x - y;
    }
}
//- /Tabs.java formatted
class Tabs {
	int f(int a,int b) {
		if (a > b)
		{
			return g (a,( a+b ) * 2);
		} else
		{
			return 0;
		}
	}

	int g(int x,int y) {
		return x-y;
	}
}
