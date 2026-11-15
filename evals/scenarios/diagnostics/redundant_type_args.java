import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

class Main {
    private final Map<String, List<Integer>> index = new HashMap/*@f1*/<String, List<Integer>>();
    private List<String> names;

    List<String> make(Collection<String> source) {
        List<String> copy = new ArrayList/*@l1*/<String>(source);
        names = new ArrayList/*@a1*/<String>();
        ArrayList<String> exact = new ArrayList/*@l2*/<String>();
        System.out.println(copy.size() + exact.size() + index.size());
        return new ArrayList/*@r1*/<String>();
    }

    void kept() {
        List<? extends Number> wild = new ArrayList<Integer>();
        Object raw = new ArrayList<String>();
        var inferred = new ArrayList<String>();
        Comparator<String> anon = new Comparator<String>() {
            public int compare(String a, String b) {
                return a.length() - b.length();
            }
        };
        List<String> diamond = new ArrayList<>();
        Function<String, Integer> f = s -> new ArrayList<String>().size();
        System.out.println(wild.size() + raw.hashCode() + inferred.size() + anon.hashCode() + diamond.size() + f.hashCode());
    }
}
//? diag redundant-type-arguments f1
//? diag redundant-type-arguments l1
//? diag redundant-type-arguments a1
//? diag redundant-type-arguments l2
//? diag redundant-type-arguments r1
//? diag only redundant-type-arguments
//? assist replace_with_diamond f1 ; has "index = new HashMap<>();"
//? assist replace_with_diamond l1 ; has "copy = new ArrayList<>(source);"
