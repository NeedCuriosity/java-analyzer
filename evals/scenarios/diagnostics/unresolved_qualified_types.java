import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;

class Main {
    static class Inner {}

    java.util./*@pkg_missing*/Missing field;
    Main./*@member_missing*/Nope other;
    Map./*@entry_missing*/Entri<String, String> entry;
    java./*@sub_missing*/utill.List<String> wrongPackage;

    java.util.List<String> ok1;
    Map.Entry<String, Integer> ok2;
    Main.Inner ok3;
    java.util.Map.Entry<String, String> ok4;
    unknown.pkg.Thing ok5;
    ArrayList<String> ok6;

    void run() {
        Comparator<String> order = new Comparator<>() {
            /*@diamond_body*/Widget widget;

            @Override
            public int compare(String a, String b) {
                /*@diamond_local*/Gadget g = null;
                return a.compareTo(b);
            }
        };
        List<String> names = new ArrayList<>() {
            Inner ok7;
            Main.Inner ok8;
        };
        System.out.println(order.compare("a", "b") + names.size());
    }
}
//? diag unresolved-type pkg_missing
//? diag unresolved-type member_missing
//? diag unresolved-type entry_missing
//? diag unresolved-type sub_missing
//? diag unresolved-type diamond_body
//? diag unresolved-type diamond_local
//? diag only unresolved-type
