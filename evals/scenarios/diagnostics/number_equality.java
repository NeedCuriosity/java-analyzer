import java.util.Map;

class Main {
    boolean compare(Integer a, Integer b, int c, Long d, Boolean e, Boolean f, Map<String, Integer> counts) {
        boolean x = a /*@eq*/== b;
        boolean y = a == c;
        boolean z = d == null;
        boolean w = e == f;
        boolean v = counts.get("a") /*@ne*/!= counts.get("b");
        return x && y && z && w && v;
    }
}
//? diag number-equality eq
//? diag number-equality ne
//? diag only number-equality
//? assist replace_with_equals eq ; has "boolean x = a.equals(b);"
//? assist replace_with_equals ne ; has "boolean v = !counts.get(\"a\").equals(counts.get(\"b\"));"
