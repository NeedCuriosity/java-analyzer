import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.function.Function;

interface /*@actionIface*/Action<T> {
    T /*@run*/run();
}

class Main {
    static <T> T doIt(Action<T> action) { return action.run(); }

    int size(List<String> names) {
        Integer /*@result*/result = doIt(new /*@actionNew*/Action<>() {
            int /*@countDecl*/count;
            public Integer /*@runImpl*/run() {
                /*@countUse*/count++;
                return names.size() + /*@helperCall*/helper();
            }
            int /*@helperDecl*/helper() { return /*@countRead*/count; }
        });
        return result;
    }

    void sort(List<String> names) {
        names.sort(new Comparator<>() {
            @Override
            public int compare(String a, String b) {
                return a./*@strComp*/length() - b.length();
            }
        });
        Function<String, Integer> f = new Function<>() {
            public Integer apply(String s) { return s.length(); }
        };
        List<Integer> sizes = new ArrayList<>() {
            { add(f.apply("x")); }
        };
        sizes./*@listAdd*/add(1);
    }
}
//? def countUse -> countDecl
//? def countRead -> countDecl
//? def helperCall -> helperDecl
//? refs countDecl = countUse countRead
//? impl run -> runImpl
//? impl actionIface -> actionNew
//? rename run execute ; has "public Integer execute() {" ; has "return action.execute();"
//? hover result ~ "Integer result"
//? hover strComp ~ "int length()"
//? complete strComp has length isEmpty

//? diag none
