import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

class Main {
    void run(Map<String, List<Integer>> index, String[] words) {
        var /*@list*/list = new ArrayList<String>();
        var /*@diamond*/diamond = new HashMap<String, Integer>();
        var /*@entry*/entry = index.entrySet().iterator().next();
        var /*@value*/value = entry.getValue();
        var /*@elem*/elem = value.get(0);
        var /*@word*/word = words[0];
        var /*@len*/len = word.length();
        var /*@chars*/chars = word.toCharArray();
        var /*@text*/text = "a" + len;
        var /*@ratio*/ratio = len / 2.0;
        var /*@big*/big = len + 1L;
        var /*@flag*/flag = len > 1 && !word.isEmpty();
        var /*@ch*/ch = word.charAt(0);
        var /*@keys*/keys = index.keySet();
        for (var /*@item*/item : value) {
            System.out.println(item);
        }
        for (var /*@e2*/e2 : index.entrySet()) {
            System.out.println(e2.getKey());
        }
    }
}
//? hover list ~ "ArrayList<String> list"
//? hover diamond ~ "HashMap<String, Integer> diamond"
//? hover entry ~ "Map.Entry<String, List<Integer>> entry"
//? hover value ~ "List<Integer> value"
//? hover elem ~ "Integer elem"
//? hover word ~ "String word"
//? hover len ~ "int len"
//? hover chars ~ "char[] chars"
//? hover text ~ "String text"
//? hover ratio ~ "double ratio"
//? hover big ~ "long big"
//? hover flag ~ "boolean flag"
//? hover ch ~ "char ch"
//? hover keys ~ "Set<String> keys"
//? hover item ~ "Integer item"
//? hover e2 ~ "Map.Entry<String, List<Integer>> e2"
//? diag none
