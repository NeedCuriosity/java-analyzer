import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

class Main {
    void run(List<String> names, String[] array, String text, Stream<String> stream) {
        boolean any = names.stream().filter(n -> n.isEmpty()).findFirst()./*@any*/isPresent();
        boolean none = names.stream().filter(String::isBlank).findAny()./*@none*/isEmpty();
        names./*@each*/stream().forEach(System.out::println);
        long count = Arrays./*@arr*/asList(array).stream().count();
        List<String> same = stream./*@map*/map(s -> s).collect(Collectors.toList());
        Collections./*@sort*/sort(names);
        Collections./*@sort2*/sort(names, Comparator.reverseOrder());
        if (/*@idx*/text.indexOf("x") != -1) {
            System.out.println("x");
        }
        if (/*@idx2*/names.indexOf("a") < 0) {
            System.out.println("a");
        }
        String s = "n=" + /*@vo*/String.valueOf(count);
        System.out.println(any + " " + none + same + s);
    }

    void fine(List<String> names, String text, char[] chars, Stream<Integer> ints) {
        // 位置有用的 indexOf、字符参数、char[]、数值拼接、类型变化的 map：都不报
        int at = text.indexOf("x");
        boolean hasChar = text.indexOf('c') >= 0;
        String fromChars = "c=" + String.valueOf(chars);
        String number = String.valueOf(1) + 2;
        List<Long> longs = ints.map(i -> (long) i).collect(Collectors.toList());
        names.forEach(System.out::println);
        List<String> copy = new ArrayList<>(Arrays.asList("a", "b"));
        System.out.println(at + " " + hasChar + fromChars + number + longs + copy);
    }
}

//? diag stream-api-chain any
//? diag stream-api-chain none
//? diag stream-api-chain each
//? diag stream-api-chain arr
//? diag stream-api-chain map
//? diag collections-sort sort
//? diag collections-sort sort2
//? diag index-of-contains idx
//? diag index-of-contains idx2
//? diag redundant-value-of vo
//? diag only stream-api-chain
//? diag only collections-sort
//? diag only index-of-contains
//? diag only redundant-value-of
//? assist fix_redundancy any ; has "boolean any = names.stream().anyMatch(n -> n.isEmpty());"
//? assist fix_redundancy none ; has "boolean none = names.stream().noneMatch(String::isBlank);"
//? assist fix_redundancy each ; has "names.forEach(System.out::println);\n        long count"
//? assist fix_redundancy arr ; has "long count = Arrays.stream(array).count();"
//? assist fix_redundancy map ; has "List<String> same = stream.collect(Collectors.toList());"
//? assist fix_redundancy sort ; has "names.sort(null);"
//? assist fix_redundancy sort2 ; has "names.sort(Comparator.reverseOrder());"
//? assist fix_redundancy idx ; has "if (text.contains(\"x\")) {"
//? assist fix_redundancy idx2 ; has "if (!names.contains(\"a\")) {"
//? assist fix_redundancy vo ; has "String s = \"n=\" + count;"
