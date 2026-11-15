import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

class Main {
    boolean both(boolean a, boolean b, int x) {
        return a /*@dm1*/&& b && x > 0;
    }

    boolean notEither(boolean a, boolean b) {
        return !(a /*@dm2*/|| !b);
    }

    boolean nested(boolean a, boolean b, boolean c) {
        return c && !(a /*@dm3*/&& b);
    }

    boolean keepParens(boolean a, boolean b, boolean c) {
        return !(a || b) /*@dm4*/|| c;
    }

    int early(int x) {
        if (x < 0) {
            return -1;
        } /*@else1*/else {
            int y = x * 2;
            return y + 1;
        }
    }

    void loop(List<String> items) {
        for (String s : items) {
            if (s.isEmpty()) continue;
            /*@else2*/else System.out.println(s);
        }
    }

    int notAbrupt(int x) {
        if (x < 0) {
            x = 0;
        } /*@noelse1*/else {
            x++;
        }
        return x;
    }

    int conflict(int x) {
        if (x < 0) {
            return 0;
        } /*@noelse2*/else {
            int y = x;
            System.out.println(y);
        }
        int y = 3;
        return y;
    }

    void vars() {
        ArrayList<String> /*@var1*/list = new ArrayList<String>();
        String /*@var2*/name = "x" + list.size();
        int[] /*@var3*/values = new int[3];
        List<String> /*@novar1*/other = new ArrayList<String>();
        ArrayList<String> /*@novar2*/diamond = new ArrayList<>();
        List<String> /*@novar3*/empty = Collections.emptyList();
        long /*@novar4*/count = 1;
        Runnable /*@novar5*/r = () -> {};
        System.out.println(name + values.length + other + diamond + empty + count + r);
    }
}

//? assist demorgan dm1 ; has "return !(!a || !b || x <= 0);"
//? assist demorgan dm2 ; has "return !a && b;"
//? assist demorgan dm3 ; has "return c && (!a || !b);"
//? assist demorgan dm4 ; has "return !((a || b) && !c);"
//? assist remove_redundant_else else1 ; has "            return -1;\n        }\n        int y = x * 2;\n        return y + 1;\n    }"
//? assist remove_redundant_else else2 ; has "if (s.isEmpty()) continue;\n            System.out.println(s);"
//? noassist remove_redundant_else noelse1
//? noassist remove_redundant_else noelse2
//? assist replace_type_with_var var1 ; has "var list = new ArrayList<String>();"
//? assist replace_type_with_var var2 ; has "var name = \"x\" + list.size();"
//? assist replace_type_with_var var3 ; has "var values = new int[3];"
//? noassist replace_type_with_var novar1
//? noassist replace_type_with_var novar2
//? noassist replace_type_with_var novar3
//? noassist replace_type_with_var novar4
//? noassist replace_type_with_var novar5
