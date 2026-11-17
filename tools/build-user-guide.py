"""生成面向首次使用者的图文说明，界面图片来自真实桌面程序。"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
from docx import Document
from docx.shared import Inches, Pt, RGBColor
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.opc.constants import RELATIONSHIP_TYPE as RT

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'target/manual/assets'
OUT = ROOT / 'docs/MSync 图文使用说明.docx'
FONT = 'C:/Windows/Fonts/msyh.ttc'
GREEN = '#176a57'

def canvas(height):
    im = Image.new('RGB', (1440, height), 'white')
    return im, ImageDraw.Draw(im)

def text(d, xy, value, size=34, fill='#172a24'):
    d.text(xy, value, font=ImageFont.truetype(FONT, size), fill=fill)

im,d = canvas(520)
d.rounded_rectangle((55,65,1385,475),radius=20,fill='#f5f7f6',outline='#cedbd5',width=3)
text(d,(95,95),'解压后的 MSync 文件夹',38)
for y,name,detail in [(180,'MSync.exe','每天双击这个打开软件'),(260,'首次连接','第一次连接不上时再打开'),(340,'MSync 图文使用说明.docx','遇到问题时，随时翻看')]:
    d.rounded_rectangle((98,y,138,y+39),radius=5,fill=GREEN)
    text(d,(165,y-4),name,31)
    text(d,(800,y-4),detail,31, '#486357')
text(d,(95,417),'其余文件请留在原处，不用打开。',28,'#62746b')
im.save(ASSETS/'示意-打开软件.png')

im,d = canvas(560)
steps=[('1','设置 → 我的设备 → 全部参数与信息','连续点击 MIUI 版本（OS 版本）7 次'),('2','返回设置 → 更多设置 → 开发者选项','找到 USB 调试，打开右边的开关'),('3','用数据线连接电脑，解锁平板','出现允许提示时，按下图操作')]
for i,(num,title,desc) in enumerate(steps):
    y=30+i*175
    d.ellipse((35,y+10,109,y+84),fill=GREEN)
    text(d,(60,y+17),num,39,'white')
    text(d,(145,y),title,37)
    text(d,(145,y+66),desc,30,'#5c6f64')
im.save(ASSETS/'示意-平板准备.png')

im,d = canvas(440)
d.rounded_rectangle((160,20,1280,420),radius=28,fill='#f7f9f8',outline='#bacdc3',width=3)
text(d,(220,65),'允许 USB 调试吗',43)
d.rectangle((222,163,254,195),outline=GREEN,width=3)
d.line((226,177,235,187,251,167),fill=GREEN,width=4)
text(d,(278,153),'始终允许使用这台计算机进行调试',33)
text(d,(240,294),'取消',34,'#62746b')
d.rounded_rectangle((825,271,1190,352),radius=15,fill=GREEN)
text(d,(955,284),'允许',37,'white')
im.save(ASSETS/'示意-允许连接.png')

im,d = canvas(330)
for x,label in [(55,'这台电脑'),(568,'平板'),(1085,'另一台电脑')]:
    d.rounded_rectangle((x,30,x+300,192),radius=20,fill='#edf5f1',outline='#a9c5b8',width=3)
    text(d,(x+35,84),label,38)
for x in [390,905]:
    d.line((x,85,x+135,85),fill=GREEN,width=6)
    d.polygon([(x+135,85),(x+113,71),(x+113,99)],fill=GREEN)
    d.line((x,128,x+135,128),fill=GREEN,width=6)
    d.polygon([(x,128),(x+22,114),(x+22,142)],fill=GREEN)
text(d,(42,240),'MSync 负责电脑与平板',29)
text(d,(826,240),'FolderSync 负责后半程',29)
im.save(ASSETS/'示意-接力同步.png')

doc=Document()
s=doc.sections[0]
s.page_width=Inches(8.5);s.page_height=Inches(11)
s.top_margin=Inches(.65);s.bottom_margin=Inches(.65)
s.left_margin=Inches(.7);s.right_margin=Inches(.7)
s.footer_distance=Inches(.28)
for name in ['Normal','Title','Subtitle','Heading 1','Heading 2','Caption']:
    st=doc.styles[name];st.font.name='Microsoft YaHei';st.font.color.rgb=RGBColor(0,0,0)
    st.element.get_or_add_rPr().get_or_add_rFonts().set(qn('w:eastAsia'),'Microsoft YaHei')
    for key in ('asciiTheme','hAnsiTheme','eastAsiaTheme','cstheme'):
        st.element.get_or_add_rPr().get_or_add_rFonts().attrib.pop(qn('w:'+key),None)
    for border in st.element.xpath('.//w:pBdr'):
        border.getparent().remove(border)
    snap=OxmlElement('w:snapToGrid');snap.set(qn('w:val'),'0');st.element.get_or_add_pPr().append(snap)
for grid in s._sectPr.xpath('./w:docGrid'):grid.getparent().remove(grid)
doc.styles['Normal'].font.size=Pt(12)
doc.styles['Normal'].paragraph_format.line_spacing=Pt(18)
doc.styles['Normal'].paragraph_format.space_after=Pt(7)
for name,size in [('Title',27),('Heading 1',22),('Heading 2',14)]:
    doc.styles[name].font.size=Pt(size)
    doc.styles[name].paragraph_format.space_before=Pt(8)
    doc.styles[name].paragraph_format.space_after=Pt(8)
    doc.styles[name].paragraph_format.line_spacing=1.15
doc.styles['Caption'].font.size=Pt(10)
doc.styles['Caption'].font.italic=False
doc.styles['Caption'].font.bold=False
doc.styles['Caption'].paragraph_format.line_spacing=Pt(13)
doc.styles['Caption'].paragraph_format.space_after=Pt(8)
footer=s.footer.paragraphs[0];footer.alignment=WD_ALIGN_PARAGRAPH.RIGHT
footer.add_run('MSync 使用说明  ·  ').font.size=Pt(9)
field=OxmlElement('w:fldSimple');field.set(qn('w:instr'),'PAGE');footer._p.append(field)
doc.core_properties.title='MSync 图文使用说明'
doc.core_properties.subject='首次连接与日常同步'
doc.core_properties.author='MSync'

def p(value,bold=False):
    para=doc.add_paragraph();para.add_run(value).bold=bold;return para
def h(value):doc.add_heading(value,2)
def page(title):doc.add_heading(title,1).paragraph_format.page_break_before=True
def picture(name,caption,width=6.8,crop=None):
    file=ASSETS/(name+'.png');w,h=Image.open(file).size
    para=doc.add_paragraph();para.alignment=WD_ALIGN_PARAGRAPH.CENTER
    para.paragraph_format.space_after=Pt(5);para.paragraph_format.keep_with_next=True
    para.paragraph_format.line_spacing=1.0
    shape=para.add_run().add_picture(str(file),width=Inches(width))
    if crop:
        l,t,r,b=crop
        src=OxmlElement('a:srcRect')
        for k,v in zip(('l','t','r','b'),crop):src.set(k,str(round(v*100000)))
        shape._inline.graphic.graphicData.pic.blipFill.insert(1,src)
        shape.height=int(Inches(width)*(h*(1-t-b))/(w*(1-l-r)))
    shape._inline.docPr.set('descr',caption)
    cap=doc.add_paragraph(caption,'Caption');cap.alignment=WD_ALIGN_PARAGRAPH.CENTER

doc.add_heading('MSync 图文使用说明',0)
p('第一次照着设置，以后打开软件、接上平板即可同步。')
p('适用软件 0.2.6　｜　Windows 电脑与小米平板',False)
h('先准备好这三样')
p('收到的完整软件压缩包、平板，以及一根能传文件的数据线。电脑不需要联网。平板与另一台电脑之间的同步，应已由提供软件的人设置好。')
h('先把软件完整解压')
p('1　用 7-Zip 打开收到的 .7z 压缩包，完整解压到一个文件夹。')
p('2　打开解压出来的文件夹，双击 MSync.exe。第一次打开可能需要稍等。')
picture('示意-打开软件','文件夹示意　文件名后面的 .exe 可能不显示')
p('请保留整个软件文件夹。下次仍从这里打开 MSync，不要只把一个图标单独拷走。')
h('按顺序看这几页')
p('第 2 页：准备平板　　第 3—4 页：选择文件夹\n第 5 页：第一次同步　　第 6 页：每天怎么用\n第 7 页：连接不上或遇到问题')

page('1  让平板允许电脑连接')
p('这部分通常只做一次。先解锁平板，打开“设置”。如果已经开启 USB 调试，可以直接接线并查看授权提示。')
picture('示意-平板准备','操作示意　系统版本不同，菜单名称和位置可能略有差别')
p('在“全部参数与信息”中连续点击 MIUI 版本（OS 版本）7 次。有些系统把“我的设备”显示为“关于平板”，把“开发者选项”显示为“开发人员选项”。需要密码时，输入平板的解锁密码。')
picture('示意-允许连接','授权提示示意　勾选后点“允许”',5.8)
p('只对自己要使用的这台电脑点“允许”。随后回到 MSync 继续操作。若平板没有弹出提示，或软件看不到平板，请看第 7 页。')

page('2  选择要同步的电脑文件夹')
p('文件可以继续打开和编辑。第一次建议用少量测试文件，确认选对文件夹后再放入正式资料。')
p('点击“电脑”右边的“选择”，找到电脑上要同步的工作文件夹，再点弹出窗口里的“选择文件夹”。',True)
picture('01-首页','软件首页　上面选电脑文件夹，下面选平板文件夹',6.0)
p('选中的文件夹，以及里面的子文件夹都会参与同步。请只选工作资料所在的那一个文件夹。')

page('3  选择平板上的工作文件夹')
p('1　点击“平板”右边的“选择”，再点击列表中的平板名称。')
picture('02-选择平板','界面示例　点击已连接的平板',5.2)
p('2　打开平板里的“工作文件”。')
p('3　确认上方路径以“工作文件”结尾，再点“选择此文件夹”。选错层级可点左侧返回箭头。')
picture('04-确认文件夹','界面示例　选择平板上的工作文件夹',5.2)
p('“此处没有子文件夹”只表示没有下一层文件夹，不代表里面没有文件，也不妨碍选择。')

page('4  完成第一次同步')
p('1　回到首页，核对上面的电脑文件夹和下面的平板文件夹。确认后点绿色的“开始同步”。')
p('2　软件会直接合并两边文件。等到出现“已完成”，并且“上次成功”变为这次的时间。')
picture('05-首次同步','选好目录后直接同步，完成时显示本次成功时间',5.8)
p('同步时可以继续使用文件。检查阶段显示动画；传输时显示已处理数量、进度和已用时间，结束后保留本轮用时。正在保存或被占用的文件会稍后重试。')
h('第一次确认两边都能收到')
p('在电脑工作文件夹放入一份小文件，点“立即同步”，完成后去平板的同一文件夹查看。再在平板放入另一份小文件，同步后回电脑查看。')
p('电脑和平板会互相更新。以后删除其中一边的文件，另一边也可能跟着删除；被替换或删除的旧文件会保存到备份。',True)
p('如果弹出“选择保留的版本”，左右分别显示电脑版和平板版。可以先点各自的“打开”查看副本，再勾选要保留的一版，点击“同步所选版本”。另一版会存入备份。不确定时点“稍后处理”或按 Esc，其他文件照常同步；首页点“处理冲突”可以再次打开。')

page('5  以后每天这样用')
picture('示意-接力同步','文件分两段到达另一台电脑，返回时顺序相反',6.2)
h('把这台电脑的修改带到另一台电脑')
p('保存文件 → 打开 MSync → 接线并解锁平板 → 点“立即同步”。等到本次“已完成”后再拔线。')
p('随后让 FolderSync 把平板的“工作文件”同步到另一台电脑。MSync 只负责这台电脑与平板。')
h('把另一台电脑的修改带回来')
p('先让另一台电脑的修改通过 FolderSync 到达平板。等它完成后，再连接这台电脑，用 MSync 同步。')
p('同步期间可以继续工作；两台电脑同时改同一份文件时，可能需要你选择保留的版本。')
h('软件可以自动检查')
picture('07-更多设置','点击首页右下角“···”打开设置',1.8)
p('保持“自动同步”开启，软件空闲时会大约每分钟检查一次。着急时，直接点“立即同步”。')
p(r'“忽略文件”每行填写一条正则表达式，匹配文件名。默认只有 ^~\$，跳过 ~$ 开头的文件。保存后下次同步生效，已有文件保留原样；留空后恢复同步。')
p('每天先打开软件。关闭窗口会收起到托盘，需要选版时自动弹出。完全退出：托盘右键 MSync → 退出。')
p('工作目录被移走或删除后，会在原位置重建并从另一端补回。内部标记异常会自动合并恢复，无需重新绑定。')

page('6  遇到问题照这里做')
h('软件里看不到平板')
p('先解锁平板，确认 USB 调试已打开，拔插一次数据线，再点“刷新”。可以换一个电脑插口或换一根能传文件的数据线。')
p('仍然没有时，打开软件文件夹里的“首次连接”，双击“首次连接.cmd”。若电脑询问是否允许更改，点“是”；看到完成提示后，查看平板上的“允许”提示，再回软件点“刷新”。')
p('如果要求管理员密码、提示未完成，或仍然无法连接，请把提示截图发给提供软件的人。不同电脑的首次准备可能需要协助。')
h('显示等待授权或存储暂不可访问')
p('解锁平板。若出现“允许 USB 调试”，勾选“始终允许使用这台计算机进行调试”并点“允许”，再回软件重试。平板重启后，先输入一次密码解锁。')
h('断线或部分文件未完成')
p('重新接线并解锁平板，软件会自动继续。单个文件重试三次仍失败，会显示原因，其余文件继续同步；展开“项未完成”查看。手动停止后，点“立即同步”继续。')
h('复制日志发给协助你的人')
p('在首页点击“查看日志”，再点“复制日志”。打开聊天窗口，右键选择“粘贴”并发送，同时说明刚才做了什么、卡在哪一步。你不用看懂日志。')
p('日志会自动清理：单文件最多 10 MB，总量最多 100 MB，保留最近 7 天。同步记录和备份不受影响。')
picture('08-复制日志','点击“复制日志”　日志可能包含文件夹和文件名称',6.6)
h('误删文件或想找回旧版')
p('先暂停操作，点击首页“打开备份”。如果不知道该找哪一份，把日志发给提供软件的人，请他协助恢复。不要自己清空备份文件夹。')
p('平板设置参考：',False)
link=OxmlElement('w:hyperlink');link.set(qn('r:id'),doc.part.relate_to('https://www.mi.com/global/support/faq/details/KA-168765/',RT.HYPERLINK,is_external=True))
run=OxmlElement('w:r');rp=OxmlElement('w:rPr');size=OxmlElement('w:sz');size.set(qn('w:val'),'20');rp.append(size);run.append(rp);txt=OxmlElement('w:t');txt.text='打开小米官方的平板设置说明';run.append(txt);link.append(run);doc.paragraphs[-1]._p.append(link)
doc.save(OUT)
print(OUT)
